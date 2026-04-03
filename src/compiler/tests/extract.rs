use std::collections::HashMap;
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
    metadata: Option<Metadata>,
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
    metadata: Option<Metadata>,
}

#[derive(Debug, Clone)]
struct MockTransclusion {
    target: String,
    metadata: Option<Metadata>,
}

#[derive(Debug)]
struct ExpectedOutput {
    title: String,
    node_metadata: Metadata,
    transclusions: Vec<String>,
    links: Vec<String>,
    transclusion_metadata: HashMap<u32, Metadata>,
    link_metadata: HashMap<u32, Metadata>,
}

impl MockFile {
    fn assign_unique_identifiers(&mut self) {
        let mut next_id = 0u32;
        let mut stack = vec![&mut self.primary];

        while let Some(node) = stack.pop() {
            node.identifier = format!("n{next_id}");
            next_id = next_id
                .checked_add(1)
                .expect("node identifier counter overflow");

            for element in node.body.iter_mut().rev() {
                if let MockElement::Subnode(subnode) = element {
                    stack.push(&mut subnode.node);
                }
            }
        }
    }

    fn render(&self) -> (FileOutput, HashMap<String, ExpectedOutput>) {
        let mut html = String::new();
        let mut spans = HashMap::new();
        let mut file_node_metadata = HashMap::new();
        let mut file_transclusion_metadata = HashMap::new();
        let mut file_link_metadata = HashMap::new();
        let mut synthetic_transclusions: Vec<(String, Option<Metadata>)> = Vec::new();
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
                node_metadata: &'a Option<Metadata>,
            },
        }

        struct EdgeInfo {
            owner_identifier: String,
            transclusions: Vec<String>,
            links: Vec<String>,
            transclusion_metadata: HashMap<u32, Metadata>,
            link_metadata: HashMap<u32, Metadata>,
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
                    write!(html, r#"<{tag} identifier="{}""#, node.identifier).unwrap();
                    if let Some(transclude) = option_transclude {
                        let value = if transclude { "true" } else { "false" };
                        write!(html, r#" transclude="{value}""#).unwrap();
                    }
                    html.push('>');
                    write!(html, "<wb-title>{}</wb-title>", node.title).unwrap();

                    stack.push(Work::Close {
                        identifier: &node.identifier,
                        tag,
                        title: &node.title,
                        node_metadata: &node.metadata,
                    });
                    stack.extend(node.body.iter().rev().map(Work::Element));

                    edge_stack.push(EdgeInfo {
                        owner_identifier: node.identifier.clone(),
                        transclusions: Vec::new(),
                        links: Vec::new(),
                        transclusion_metadata: HashMap::new(),
                        link_metadata: HashMap::new(),
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
                            if let Some(metadata) = &link.metadata {
                                assert!(
                                    file_link_metadata
                                        .insert(counter, metadata.clone())
                                        .is_none(),
                                    "duplicate link metadata: {counter}",
                                );
                                assert!(
                                    edges
                                        .link_metadata
                                        .insert(counter, metadata.clone())
                                        .is_none(),
                                    "duplicate expected link metadata: {counter}",
                                );
                            }
                            edges.links.push(link.target.clone());
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
                            if let Some(metadata) = &t.metadata {
                                assert!(
                                    file_transclusion_metadata
                                        .insert(counter, metadata.clone())
                                        .is_none(),
                                    "duplicate transclusion metadata: {counter}",
                                );
                                assert!(
                                    edges
                                        .transclusion_metadata
                                        .insert(counter, metadata.clone())
                                        .is_none(),
                                    "duplicate expected transclusion metadata: {counter}",
                                );
                            }
                            edges.transclusions.push(t.target.clone());
                        }
                        MockElement::Subnode(subnode) => {
                            if subnode.transclude {
                                edges.transclusions.push(subnode.node.identifier.clone());
                                synthetic_transclusions.push((
                                    edges.owner_identifier.clone(),
                                    subnode.node.metadata.clone(),
                                ));
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
                    if let Some(metadata) = node_metadata {
                        assert!(
                            file_node_metadata
                                .insert(identifier.to_owned(), metadata.clone())
                                .is_none(),
                            "duplicate node metadata: {identifier}",
                        );
                    }

                    let edges = edge_stack.pop().unwrap();
                    expected.insert(
                        identifier.to_owned(),
                        ExpectedOutput {
                            title: title.to_owned(),
                            node_metadata: node_metadata.clone().unwrap_or_default(),
                            transclusions: edges.transclusions,
                            links: edges.links,
                            transclusion_metadata: edges.transclusion_metadata,
                            link_metadata: edges.link_metadata,
                        },
                    );
                }
            }
        }

        let mut synthetic_counter = transclusion_counter;
        for (owner_identifier, metadata) in synthetic_transclusions.into_iter().rev() {
            let counter = synthetic_counter;
            synthetic_counter = synthetic_counter
                .checked_add(1)
                .expect("synthetic transclusion counter overflow");

            if let Some(metadata) = metadata {
                assert!(
                    expected
                        .get_mut(&owner_identifier)
                        .expect("bug: missing expected owner for synthetic transclusion")
                        .transclusion_metadata
                        .insert(counter, metadata)
                        .is_none(),
                    "duplicate expected synthetic transclusion metadata: {counter}",
                );
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
            proptest::option::of(metadata_strategy()),
        )
            .prop_map(|(target, content, metadata)| MockElement::Link(MockLink {
                target,
                content,
                metadata,
            })),
        (
            "[a-z][a-z0-9]{0,7}",
            proptest::option::of(metadata_strategy())
        )
            .prop_map(|(target, metadata)| {
                MockElement::Transclusion(MockTransclusion { target, metadata })
            }),
    ]
}

fn node_strategy(body: impl Strategy<Value = Vec<MockElement>>) -> impl Strategy<Value = MockNode> {
    (
        "[A-Za-z ]{0,12}",
        proptest::option::of(metadata_strategy()),
        body,
    )
        .prop_map(|(title, metadata, body)| MockNode {
            identifier: String::new(),
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

fn mock_file_strategy() -> impl Strategy<Value = MockFile> {
    node_strategy(proptest::collection::vec(
        element_strategy(),
        0..=BODY_ELEMENTS_MAX,
    ))
    .prop_map(|primary| {
        let mut file = MockFile { primary };
        file.assign_unique_identifiers();
        file
    })
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
            prop_assert_eq!(&actual.entry.transclusion_metadata, &expected.transclusion_metadata);
            prop_assert_eq!(&actual.entry.link_metadata, &expected.link_metadata);

            let document = Document::from(actual.entry.body_html.as_str());

            prop_assert!(document.select("wb-subnode").iter().next().is_none());
            prop_assert!(document.select("wb-title").iter().next().is_none());
        }
    }
}
