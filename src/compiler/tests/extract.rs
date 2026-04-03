use std::collections::HashMap;
use std::fmt::Write;

use proptest::prelude::*;
use typst::syntax::Span;

use crate::compiler::Metadata;
use crate::compiler::extract::{FileOutput, extract};

const METADATA_VEC_COUNT_MAX: usize = 10;
const METADATA_ENTRIES_COUNT_MAX: usize = 16;
const BODY_ELEMENTS_MAX: usize = 16;
const BODY_DEPTH: u32 = 4;
const BODY_DESIRED_SIZE: u32 = 32;
const BODY_BRANCH_SIZE: u32 = 8;

#[derive(Debug, Clone)]
struct MockFile {
    primary: MockNode,
}

#[derive(Debug, Clone)]
struct MockNode {
    identifier: String,
    title: String,
    metadata: Metadata,
    body: Vec<MockElement>,
}

#[derive(Debug, Clone)]
enum MockElement {
    Text(String),
    Link(MockLink),
    Transclusion(MockTransclusion),
    Subnode(MockSubnode),
}

#[derive(Debug, Clone)]
struct MockSubnode {
    node: MockNode,
    transclude: bool,
}

#[derive(Debug, Clone)]
struct MockLink {
    target: String,
    content: Option<String>,
    metadata: Metadata,
}

#[derive(Debug, Clone)]
struct MockTransclusion {
    target: String,
    metadata: Metadata,
}

impl MockFile {
    fn render(&self) -> FileOutput {
        let mut html = String::new();
        let mut spans = HashMap::new();
        let mut node_metadata = HashMap::new();
        let mut transclusion_metadata = HashMap::new();
        let mut link_metadata = HashMap::new();
        let mut transclusion_counter = 0u32;
        let mut link_counter = 0u32;

        render_node(
            &self.primary,
            "wb-node",
            None,
            &mut html,
            &mut spans,
            &mut node_metadata,
            &mut transclusion_metadata,
            &mut link_metadata,
            &mut transclusion_counter,
            &mut link_counter,
        );

        FileOutput {
            html,
            spans,
            node_metadata,
            transclusion_metadata,
            link_metadata,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_node(
    node: &MockNode,
    tag: &str,
    transclude: Option<bool>,
    html: &mut String,
    spans: &mut HashMap<String, Span>,
    node_metadata: &mut HashMap<String, Metadata>,
    transclusion_metadata: &mut HashMap<u32, Metadata>,
    link_metadata: &mut HashMap<u32, Metadata>,
    transclusion_counter: &mut u32,
    link_counter: &mut u32,
) {
    // Open tag
    write!(html, r#"<{tag} identifier="{}""#, node.identifier).unwrap();
    if let Some(tc) = transclude {
        let tc = if tc { "true" } else { "false" };
        write!(html, r#" transclude="{tc}""#).unwrap();
    }
    html.push('>');

    // Title
    write!(html, "<wb-title>{}</wb-title>", node.title).unwrap();

    // Body elements
    for element in &node.body {
        render_element(
            element,
            html,
            spans,
            node_metadata,
            transclusion_metadata,
            link_metadata,
            transclusion_counter,
            link_counter,
        );
    }

    // Close tag
    write!(html, "</{tag}>").unwrap();

    // Register span and metadata
    assert!(
        spans
            .insert(node.identifier.clone(), Span::detached())
            .is_none(),
        "duplicate span: {}",
        node.identifier,
    );
    assert!(
        node_metadata
            .insert(node.identifier.clone(), node.metadata.clone())
            .is_none(),
        "duplicate node metadata: {}",
        node.identifier,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_element(
    element: &MockElement,
    html: &mut String,
    spans: &mut HashMap<String, Span>,
    node_metadata: &mut HashMap<String, Metadata>,
    transclusion_metadata: &mut HashMap<u32, Metadata>,
    link_metadata: &mut HashMap<u32, Metadata>,
    transclusion_counter: &mut u32,
    link_counter: &mut u32,
) {
    match element {
        MockElement::Text(text) => {
            write!(html, "<p>{text}</p>").unwrap();
        }
        MockElement::Link(link) => {
            let counter = *link_counter;
            *link_counter += 1;
            let content = link.content.as_deref().unwrap_or_default();
            write!(
                html,
                r#"<a href="wb:{}" data-counter="{counter}">{content}</a>"#,
                link.target,
            )
            .unwrap();
            assert!(
                link_metadata
                    .insert(counter, link.metadata.clone())
                    .is_none(),
                "duplicate link metadata: {counter}",
            );
        }
        MockElement::Transclusion(t) => {
            let counter = *transclusion_counter;
            *transclusion_counter += 1;
            write!(
                html,
                r#"<wb-transclude identifier="{}" counter="{counter}"></wb-transclude>"#,
                t.target,
            )
            .unwrap();
            assert!(
                transclusion_metadata
                    .insert(counter, t.metadata.clone())
                    .is_none(),
                "duplicate transclusion metadata: {counter}",
            );
        }
        MockElement::Subnode(subnode) => {
            render_node(
                &subnode.node,
                "wb-subnode",
                Some(subnode.transclude),
                html,
                spans,
                node_metadata,
                transclusion_metadata,
                link_metadata,
                transclusion_counter,
                link_counter,
            );
        }
    }
}

impl MockFile {
    /// Returns the total number of nodes (primary + all subnodes, recursively).
    fn node_count(&self) -> usize {
        1 + count_subnodes(&self.primary.body)
    }

    /// Collects all nodes as (identifier, &MockNode, is_subnode, transclude).
    fn all_nodes(&self) -> Vec<(&str, &MockNode, bool, Option<bool>)> {
        let mut result = vec![(self.primary.identifier.as_str(), &self.primary, false, None)];
        collect_nodes(&self.primary.body, &mut result);
        result
    }
}

fn count_subnodes(body: &[MockElement]) -> usize {
    body.iter()
        .map(|e| match e {
            MockElement::Subnode(s) => 1 + count_subnodes(&s.node.body),
            _ => 0,
        })
        .sum()
}

fn collect_nodes<'a>(
    body: &'a [MockElement],
    result: &mut Vec<(&'a str, &'a MockNode, bool, Option<bool>)>,
) {
    for element in body {
        if let MockElement::Subnode(s) = element {
            result.push((
                s.node.identifier.as_str(),
                &s.node,
                true,
                Some(s.transclude),
            ));
            collect_nodes(&s.node.body, result);
        }
    }
}

/// For a given node, returns the expected transclusion targets: explicit
/// transclusions in the body plus transcluding direct child subnodes.
fn expected_transclusions(node: &MockNode) -> Vec<&str> {
    let mut result = Vec::new();
    for element in &node.body {
        match element {
            MockElement::Transclusion(t) => result.push(t.target.as_str()),
            MockElement::Subnode(s) if s.transclude => {
                result.push(s.node.identifier.as_str());
            }
            _ => {}
        }
    }
    result
}

/// For a given node, returns the expected link targets.
fn expected_links(node: &MockNode) -> Vec<&str> {
    node.body
        .iter()
        .filter_map(|e| match e {
            MockElement::Link(l) => Some(l.target.as_str()),
            _ => None,
        })
        .collect()
}

fn metadata_strategy() -> impl Strategy<Value = Metadata> {
    proptest::collection::hash_map(
        "[a-z]+",
        proptest::collection::vec("[a-z0-9]+", 0..=METADATA_VEC_COUNT_MAX),
        0..=METADATA_ENTRIES_COUNT_MAX,
    )
}

fn leaf_element_strategy() -> impl Strategy<Value = MockElement> {
    prop_oneof![
        "[a-z ]{1,15}".prop_map(MockElement::Text),
        (
            "[a-z][a-z0-9]{0,7}",
            proptest::option::of("[a-z ]{1,10}"),
            metadata_strategy(),
        )
            .prop_map(|(target, content, metadata)| MockElement::Link(MockLink {
                target,
                content,
                metadata,
            })),
        ("[a-z][a-z0-9]{0,7}", metadata_strategy()).prop_map(|(target, metadata)| {
            MockElement::Transclusion(MockTransclusion { target, metadata })
        }),
    ]
}

fn node_strategy(body: impl Strategy<Value = Vec<MockElement>>) -> impl Strategy<Value = MockNode> {
    (
        "[a-z0-9]{1,7}",
        "[A-Za-z ]{0,12}",
        metadata_strategy(),
        body,
    )
        .prop_map(|(identifier, title, metadata, body)| MockNode {
            identifier,
            title,
            metadata,
            body,
        })
}

fn element_strategy() -> impl Strategy<Value = MockElement> {
    leaf_element_strategy().prop_recursive(
        BODY_DEPTH,
        BODY_DESIRED_SIZE,
        BODY_BRANCH_SIZE,
        |inner| {
            prop_oneof![
                // Keep leaf cases so not every element is a subnode
                3 => leaf_element_strategy(),
                // Recursive case: a subnode whose body can contain deeper elements
                1 => (
                    node_strategy(proptest::collection::vec(inner, 0..=BODY_ELEMENTS_MAX)),
                    proptest::bool::ANY,
                )
                    .prop_map(|(node, transclude)| MockElement::Subnode(MockSubnode {
                        node,
                        transclude,
                    })),
            ]
        },
    )
}

/// Generates a well-formed `MockFile` with unique identifiers.
fn mock_file_strategy() -> impl Strategy<Value = MockFile> {
    node_strategy(proptest::collection::vec(
        element_strategy(),
        0..=BODY_ELEMENTS_MAX,
    ))
    .prop_map(|primary| MockFile { primary })
    .prop_filter("unique node identifiers", |file| {
        let mut seen = std::collections::HashSet::new();
        file.all_nodes()
            .iter()
            .all(|(id, _, _, _)| seen.insert(*id))
    })
}

proptest! {
    #[test]
    fn well_formed_produces_ok(file in mock_file_strategy()) {
        let output = file.render();
        let result = extract(output);

        prop_assert!(result.is_ok(), "extract failed: {:?}", result.err());
    }

    #[test]
    fn node_count_matches(file in mock_file_strategy()) {
        let expected = file.node_count();
        let output = file.render();
        let result = extract(output).unwrap();

        prop_assert_eq!(result.len(), expected);
    }

    #[test]
    fn titles_match(file in mock_file_strategy()) {
        let output = file.render();
        let result = super::super::extract::extract(output).unwrap();
        for (id, node, _, _) in file.all_nodes() {
            let extracted = result.get(id).unwrap_or_else(|| {
                panic!("missing node {id:?} in extract result")
            });
            prop_assert_eq!(&extracted.entry.title, &node.title);
        }
    }

    #[test]
    fn transclusion_edges_correct(file in mock_file_strategy()) {
        let output = file.render();
        let result = super::super::extract::extract(output).unwrap();
        for (id, node, _, _) in file.all_nodes() {
            let extracted = &result[id];
            let expected: Vec<&str> = expected_transclusions(node);
            let actual: Vec<&str> = extracted.transclusions.iter().map(String::as_str).collect();
            prop_assert_eq!(actual, expected, "transclusion mismatch for node {:?}", id);
        }
    }

    #[test]
    fn link_edges_correct(file in mock_file_strategy()) {
        let output = file.render();
        let result = super::super::extract::extract(output).unwrap();
        for (id, node, _, _) in file.all_nodes() {
            let extracted = &result[id];
            let expected: Vec<&str> = expected_links(node);
            let actual: Vec<&str> = extracted.links.iter().map(String::as_str).collect();
            prop_assert_eq!(actual, expected, "link mismatch for node {:?}", id);
        }
    }

    #[test]
    fn node_metadata_matches(file in mock_file_strategy()) {
        let output = file.render();
        let result = super::super::extract::extract(output).unwrap();
        for (id, node, _, _) in file.all_nodes() {
            let extracted = &result[id];
            prop_assert_eq!(
                &extracted.entry.node_metadata,
                &node.metadata,
                "metadata mismatch for node {:?}", id,
            );
        }
    }

    #[test]
    fn non_transcluding_subnodes_removed(file in mock_file_strategy()) {
        let output = file.render();
        let result = super::super::extract::extract(output).unwrap();
        for (id, _, _, _) in file.all_nodes() {
            let extracted = &result[id];
            prop_assert!(
                !extracted.entry.body_html.contains("<wb-subnode"),
                "node {id:?} body_html still contains wb-subnode",
            );
        }
    }
}
