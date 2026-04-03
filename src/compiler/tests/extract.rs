use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;

use dom_query::Document;
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

/// Expected output for a single node, produced alongside the `FileOutput`.
/// Uses `Vec<Metadata>` instead of `HashMap<u32, Metadata>` for transclusion
/// and link metadata — we compare as multisets to avoid coupling tests to
/// extract's internal counter assignment.
#[derive(Debug)]
struct ExpectedOutput {
    title: String,
    node_metadata: Metadata,
    transclusions: Vec<String>,
    links: Vec<String>,
    transclusion_metadata: Vec<Metadata>,
    link_metadata: Vec<Metadata>,
}

impl MockFile {
    /// Renders the mock file into a `FileOutput` for feeding to `extract`,
    /// and simultaneously produces the expected per-node outputs.
    fn render(&self) -> (FileOutput, HashMap<String, ExpectedOutput>) {
        let mut html = String::new();
        let mut spans = HashMap::new();
        let mut file_node_metadata = HashMap::new();
        let mut file_transclusion_metadata = HashMap::new();
        let mut file_link_metadata = HashMap::new();
        let mut transclusion_counter = 0u32;
        let mut link_counter = 0u32;
        let mut expected = HashMap::new();

        enum Work<'a> {
            /// Open a node: write its opening tag + title, then push its
            /// body elements (reversed) followed by a Close.
            Open {
                node: &'a MockNode,
                tag: &'static str,
                option_transclude: Option<bool>,
            },
            /// Process a single leaf body element.
            Element(&'a MockElement),
            /// Close the current node: write the closing tag, register
            /// span/metadata, finalize the expected output.
            Close {
                identifier: &'a str,
                tag: &'static str,
                title: &'a str,
                node_metadata: &'a Metadata,
            },
        }

        // Per-node accumulators, pushed/popped in sync with Open/Close.
        struct EdgeInfo {
            transclusions: Vec<String>,
            links: Vec<String>,
            transclusion_metadata: Vec<Metadata>,
            link_metadata: Vec<Metadata>,
        }

        let mut stack: Vec<Work> = vec![Work::Open {
            node: &self.primary,
            tag: "wb-node",
            option_transclude: None,
        }];
        let mut edge_stack: Vec<EdgeInfo> = Vec::new();

        while let Some(work) = stack.pop() {
            match work {
                Work::Open {
                    node,
                    tag,
                    option_transclude,
                } => {
                    // Write opening tag
                    write!(html, r#"<{tag} identifier="{}""#, node.identifier).unwrap();
                    if let Some(transclude) = option_transclude {
                        let value = if transclude { "true" } else { "false" };
                        write!(html, r#" transclude="{value}""#).unwrap();
                    }
                    html.push('>');
                    write!(html, "<wb-title>{}</wb-title>", node.title).unwrap();

                    // Push Close, then body elements in reverse so they
                    // process left-to-right.
                    stack.push(Work::Close {
                        identifier: &node.identifier,
                        tag,
                        title: &node.title,
                        node_metadata: &node.metadata,
                    });
                    stack.extend(node.body.iter().rev().map(Work::Element));

                    edge_stack.push(EdgeInfo {
                        transclusions: Vec::new(),
                        links: Vec::new(),
                        transclusion_metadata: Vec::new(),
                        link_metadata: Vec::new(),
                    });
                }
                Work::Element(element) => {
                    let edges = edge_stack.last_mut().unwrap();

                    match element {
                        MockElement::Text(text) => {
                            write!(html, "<p>{text}</p>").unwrap();
                        }
                        MockElement::Link(link) => {
                            let counter = link_counter;
                            link_counter += 1;
                            let content = link.content.as_deref().unwrap_or_default();
                            write!(
                                html,
                                r#"<a href="wb:{}" data-counter="{counter}">{content}</a>"#,
                                link.target,
                            )
                            .unwrap();
                            assert!(
                                file_link_metadata
                                    .insert(counter, link.metadata.clone())
                                    .is_none(),
                                "duplicate link metadata: {counter}",
                            );
                            edges.links.push(link.target.clone());
                            edges.link_metadata.push(link.metadata.clone());
                        }
                        MockElement::Transclusion(t) => {
                            let counter = transclusion_counter;
                            transclusion_counter += 1;
                            write!(
                                html,
                                r#"<wb-transclude identifier="{}" counter="{counter}"></wb-transclude>"#,
                                t.target,
                            )
                            .unwrap();
                            assert!(
                                file_transclusion_metadata
                                    .insert(counter, t.metadata.clone())
                                    .is_none(),
                                "duplicate transclusion metadata: {counter}",
                            );
                            edges.transclusions.push(t.target.clone());
                            edges.transclusion_metadata.push(t.metadata.clone());
                        }
                        MockElement::Subnode(subnode) => {
                            if subnode.transclude {
                                edges.transclusions.push(subnode.node.identifier.clone());
                                edges
                                    .transclusion_metadata
                                    .push(subnode.node.metadata.clone());
                            }
                            // Push the subnode as a new `Open` work item. It
                            // will be fully processed before we we continue
                            // with the current node's remaining elements.
                            stack.push(Work::Open {
                                node: &subnode.node,
                                tag: "wb-subnode",
                                option_transclude: Some(subnode.transclude),
                            });
                        }
                    }
                }
                Work::Close {
                    identifier,
                    tag,
                    title,
                    node_metadata,
                } => {
                    write!(html, "</{tag}>").unwrap();

                    assert!(
                        spans
                            .insert(identifier.to_owned(), Span::detached())
                            .is_none(),
                        "duplicate span: {identifier}",
                    );
                    assert!(
                        file_node_metadata
                            .insert(identifier.to_owned(), node_metadata.clone())
                            .is_none(),
                        "duplicate node metadata: {identifier}",
                    );

                    let edges = edge_stack.pop().unwrap();
                    expected.insert(
                        identifier.to_owned(),
                        ExpectedOutput {
                            title: title.to_owned(),
                            node_metadata: node_metadata.clone(),
                            transclusions: edges.transclusions,
                            links: edges.links,
                            transclusion_metadata: edges.transclusion_metadata,
                            link_metadata: edges.link_metadata,
                        },
                    );
                }
            }
        }

        let file_output = FileOutput {
            html,
            spans,
            node_metadata: file_node_metadata,
            transclusion_metadata: file_transclusion_metadata,
            link_metadata: file_link_metadata,
        };

        (file_output, expected)
    }

    /// Returns the total number of nodes (primary + all subnodes, recursively).
    fn node_count(&self) -> usize {
        1 + count_subnodes(&self.primary.body)
    }

    /// Returns true if all node identifiers in the file are unique.
    fn has_unique_identifiers(&self) -> bool {
        let mut seen = std::collections::HashSet::new();
        has_unique_ids(&self.primary, &mut seen)
    }
}

fn has_unique_ids<'a>(node: &'a MockNode, seen: &mut std::collections::HashSet<&'a str>) -> bool {
    if !seen.insert(&node.identifier) {
        return false;
    }
    node.body.iter().all(|e| match e {
        MockElement::Subnode(s) => has_unique_ids(&s.node, seen),
        _ => true,
    })
}

fn count_subnodes(body: &[MockElement]) -> usize {
    body.iter()
        .map(|e| match e {
            MockElement::Subnode(s) => 1 + count_subnodes(&s.node.body),
            _ => 0,
        })
        .sum()
}

/// Sort a `Vec<Metadata>` for multiset comparison.
fn sorted_metadata(mut v: Vec<Metadata>) -> Vec<Metadata> {
    v.sort_by(|a, b| {
        let a: std::collections::BTreeMap<_, _> = a.iter().collect();
        let b: std::collections::BTreeMap<_, _> = b.iter().collect();
        a.cmp(&b)
    });
    v
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
                3 => leaf_element_strategy(),
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
    .prop_filter("unique node identifiers", MockFile::has_unique_identifiers)
}

proptest! {
    #[test]
    fn happy_case(file in mock_file_strategy()) {
        let (file_output, expected_nodes) = file.render();
        let actual_nodes = extract(file_output).unwrap();

        prop_assert_eq!(actual_nodes.len(), expected_nodes.len());

        for (id, expected) in &expected_nodes {
            let actual = &actual_nodes[id.as_str()];

            prop_assert_eq!(&actual.entry.title, &expected.title);
            prop_assert_eq!(&actual.entry.node_metadata, &expected.node_metadata);
            prop_assert_eq!(&actual.transclusions, &expected.transclusions);
            prop_assert_eq!(&actual.links, &expected.links);

            let mut transclusion_meta_actual: Vec<BTreeMap<String, Vec<String>>> = actual
                .entry
                .transclusion_metadata
                .values()
                .cloned()
                .map(|m| m.into_iter().collect::<BTreeMap<_, _>>())
                .collect();
            transclusion_meta_actual.sort();
            let mut transclusion_meta_expected: Vec<BTreeMap<String, Vec<String>>> = expected
                .transclusion_metadata
                .iter()
                .cloned()
                .map(|m| m.into_iter().collect::<BTreeMap<_, _>>())
                .collect();
            transclusion_meta_expected.sort();
            prop_assert_eq!(transclusion_meta_actual, transclusion_meta_expected);

            let mut link_meta_actual: Vec<BTreeMap<String, Vec<String>>> = actual
                .entry
                .link_metadata
                .values()
                .cloned()
                .map(|m| m.into_iter().collect::<BTreeMap<_, _>>())
                .collect();
            link_meta_actual.sort();
            let mut link_meta_expected: Vec<BTreeMap<String, Vec<String>>> = expected
                .link_metadata
                .iter()
                .cloned()
                .map(|m| m.into_iter().collect::<BTreeMap<_, _>>())
                .collect();
            link_meta_expected.sort();
            prop_assert_eq!(link_meta_actual, link_meta_expected);

            let document = Document::from(actual.entry.body_html.as_str());

            prop_assert!(document.select("wb-subnode").iter().next().is_none());
        }
    }

    // Individual property tests kept for finer-grained failure diagnosis.

    #[test]
    fn well_formed_produces_ok(file in mock_file_strategy()) {
        let (file_output, _) = file.render();
        let result = extract(file_output);

        prop_assert!(result.is_ok(), "extract failed: {:?}", result.err());
    }

    #[test]
    fn node_count_matches(file in mock_file_strategy()) {
        let expected_count = file.node_count();
        let (file_output, _) = file.render();
        let result = extract(file_output).unwrap();

        prop_assert_eq!(result.len(), expected_count);
    }

    #[test]
    fn titles_match(file in mock_file_strategy()) {
        let (file_output, expected) = file.render();
        let result = extract(file_output).unwrap();

        for (id, exp) in &expected {
            let actual = &result[id.as_str()];
            prop_assert_eq!(&actual.entry.title, &exp.title);
        }
    }

    #[test]
    fn transclusion_edges_correct(file in mock_file_strategy()) {
        let (file_output, expected) = file.render();
        let result = extract(file_output).unwrap();

        for (id, exp) in &expected {
            let actual = &result[id.as_str()];
            prop_assert_eq!(&actual.transclusions, &exp.transclusions,
                "transclusion mismatch for node {:?}", id);
        }
    }

    #[test]
    fn link_edges_correct(file in mock_file_strategy()) {
        let (file_output, expected) = file.render();
        let result = extract(file_output).unwrap();

        for (id, exp) in &expected {
            let actual = &result[id.as_str()];
            prop_assert_eq!(&actual.links, &exp.links,
                "link mismatch for node {:?}", id);
        }
    }

    #[test]
    fn node_metadata_matches(file in mock_file_strategy()) {
        let (file_output, expected) = file.render();
        let result = extract(file_output).unwrap();

        for (id, exp) in &expected {
            let actual = &result[id.as_str()];
            prop_assert_eq!(&actual.entry.node_metadata, &exp.node_metadata,
                "metadata mismatch for node {:?}", id);
        }
    }

    #[test]
    fn transclusion_metadata_matches(file in mock_file_strategy()) {
        let (file_output, expected) = file.render();
        let result = extract(file_output).unwrap();

        for (id, exp) in &expected {
            let actual = &result[id.as_str()];
            let actual_meta = sorted_metadata(
                actual.entry.transclusion_metadata.values().cloned().collect(),
            );
            let expected_meta = sorted_metadata(exp.transclusion_metadata.clone());
            prop_assert_eq!(actual_meta, expected_meta,
                "transclusion metadata mismatch for node {:?}", id);
        }
    }

    #[test]
    fn link_metadata_matches(file in mock_file_strategy()) {
        let (file_output, expected) = file.render();
        let result = extract(file_output).unwrap();

        for (id, exp) in &expected {
            let actual = &result[id.as_str()];
            let actual_meta = sorted_metadata(
                actual.entry.link_metadata.values().cloned().collect(),
            );
            let expected_meta = sorted_metadata(exp.link_metadata.clone());
            prop_assert_eq!(actual_meta, expected_meta,
                "link metadata mismatch for node {:?}", id);
        }
    }

    #[test]
    fn non_transcluding_subnodes_removed(file in mock_file_strategy()) {
        let (file_output, expected) = file.render();
        let result = extract(file_output).unwrap();

        for id in expected.keys() {
            let actual = &result[id.as_str()];
            prop_assert!(
                !actual.entry.body_html.contains("<wb-subnode"),
                "node {:?} body_html still contains wb-subnode", id,
            );
        }
    }
}
