use std::collections::HashMap;
use std::fmt::Write;
use std::num::NonZeroU16;

use ecow::EcoVec;
use proptest::prelude::*;
use proptest_state_machine::ReferenceStateMachine;
use typst::diag::{SourceDiagnostic, Warned};
use typst::syntax::{FileId, Span};

use crate::compiler::Metadata;
use crate::compiler::NodeEntry;
use crate::compiler::extract::NodeOutput;
use crate::config::RenderConfig;

#[derive(Debug, Clone)]
pub struct MockNode {
    pub title: String,
    pub body: String,
    pub span: Span,
    pub metadata: Metadata,
    pub transclusions: Vec<MockTransclusion>,
    pub links: Vec<MockLink>,
}

#[derive(Debug, Clone)]
pub struct MockTransclusion {
    pub target: String,
    pub metadata: Metadata,
}

#[derive(Debug, Clone)]
pub struct MockLink {
    pub target: String,
    pub content: Option<String>,
    pub metadata: Metadata,
}

impl From<MockNode> for NodeOutput {
    fn from(node: MockNode) -> Self {
        let mut body_html = String::new();
        let mut transclusion_metadata: HashMap<u32, Metadata> = HashMap::new();
        let mut link_metadata: HashMap<u32, Metadata> = HashMap::new();
        let mut transclusions = Vec::new();
        let mut links = Vec::new();

        if !node.body.is_empty() {
            write!(body_html, "<p>{}</p>", node.body).unwrap();
        }

        for (counter, transclusion) in node.transclusions.into_iter().enumerate() {
            let counter = counter as u32;
            write!(
                body_html,
                r#"<wb-transclude identifier="{}" counter="{counter}"></wb-transclude>"#,
                transclusion.target,
            )
            .unwrap();
            if !transclusion.metadata.is_empty() {
                transclusion_metadata.insert(counter, transclusion.metadata);
            }
            transclusions.push(transclusion.target);
        }

        for (counter, link) in node.links.into_iter().enumerate() {
            let counter = counter as u32;
            let content = link.content.as_deref().unwrap_or_default();
            write!(
                body_html,
                r#"<a href="wb:{}" data-counter="{counter}">{content}</a>"#,
                link.target,
            )
            .unwrap();
            if !link.metadata.is_empty() {
                link_metadata.insert(counter, link.metadata);
            }
            links.push(link.target);
        }

        let entry = NodeEntry {
            body_html,
            title: node.title.clone(),
            title_text: node.title,
            span: node.span,
            node_metadata: node.metadata,
            transclusion_metadata,
            link_metadata,
        };

        NodeOutput {
            entry,
            transclusions,
            links,
        }
    }
}

pub(super) fn test_render_config() -> RenderConfig {
    let mut env = minijinja::Environment::new();
    env.add_template_owned(
        "node.html".to_owned(),
        "{{ node.body }}{{ node.backmatter }}".to_owned(),
    )
    .unwrap();
    env.add_template_owned(
        "transclusion.html".to_owned(),
        "<div class=\"transclusion\">{{ body }}</div>".to_owned(),
    )
    .unwrap();
    env.add_template_owned(
        "link.html".to_owned(),
        "<a href=\"{{ href }}\">{{ content }}</a>".to_owned(),
    )
    .unwrap();
    env.add_template_owned("backmatter.html".to_owned(), "".to_owned())
        .unwrap();

    RenderConfig {
        root_directory: "/".to_owned(),
        trailing_slash: false,
        index_node: "index".to_owned(),
        domain: "test.local".to_owned(),
        environment: env,
    }
}

fn metadata_strategy() -> impl Strategy<Value = Metadata> {
    proptest::collection::hash_map(
        "[a-z]{1,4}",
        proptest::collection::vec("[a-z0-9]{1,6}", 0..=3),
        0..=3,
    )
}

fn mock_node_strategy(targets: Vec<String>) -> BoxedStrategy<MockNode> {
    let targets_for_transclusions = targets.clone();
    let targets_for_links = targets;

    (
        "[A-Za-z ]{1,12}",
        "[a-z ]{0,20}",
        metadata_strategy(),
        proptest::collection::vec(
            (
                proptest::sample::select(targets_for_transclusions),
                metadata_strategy(),
            )
                .prop_map(|(target, metadata)| MockTransclusion { target, metadata }),
            0..=3,
        ),
        proptest::collection::vec(
            (
                proptest::sample::select(targets_for_links),
                proptest::option::of("[a-z ]{1,8}"),
                metadata_strategy(),
            )
                .prop_map(|(target, content, metadata)| MockLink {
                    target,
                    content,
                    metadata,
                }),
            0..=3,
        ),
    )
        .prop_map(|(title, body, metadata, transclusions, links)| MockNode {
            title,
            body,
            span: Span::detached(),
            metadata,
            transclusions,
            links,
        })
        .boxed()
}

#[derive(Debug, Clone)]
pub struct AbstractState {
    /// file_id (as u16) → node_identifier → MockNode
    pub files: HashMap<u16, HashMap<String, MockNode>>,
    /// Node IDs that intentionally don't exist, for dangling transclusion/link targets
    pub missing_ids: Vec<String>,
    /// Counter for generating unique file IDs (starts at 1 for NonZeroU16)
    pub next_file_id: u16,
    /// Counter for generating unique node identifiers
    pub next_node_id: u32,
}

impl AbstractState {
    pub fn new(missing_ids: Vec<String>) -> Self {
        Self {
            files: HashMap::new(),
            missing_ids,
            next_file_id: 1,
            next_node_id: 0,
        }
    }

    pub fn to_typst_file_id(raw: u16) -> FileId {
        FileId::from_raw(NonZeroU16::new(raw).expect("file id must be nonzero"))
    }

    /// All node identifiers that currently exist across all files.
    pub fn all_node_ids(&self) -> Vec<String> {
        self.files
            .values()
            .flat_map(|nodes| nodes.keys().cloned())
            .collect()
    }

    /// All valid targets for transclusions/links: existing nodes + missing IDs.
    pub fn all_targets(&self) -> Vec<String> {
        let mut targets = self.all_node_ids();
        targets.extend(self.missing_ids.iter().cloned());
        targets
    }

    /// All (file_id, node_id) pairs that currently exist.
    pub fn all_file_node_pairs(&self) -> Vec<(u16, String)> {
        self.files
            .iter()
            .flat_map(|(&fid, nodes)| nodes.keys().map(move |nid| (fid, nid.clone())))
            .collect()
    }

    /// Convert a file's nodes into the compile output for `_update`.
    pub fn compile_file(
        &self,
        file_id: u16,
    ) -> Warned<Result<HashMap<String, NodeOutput>, EcoVec<SourceDiagnostic>>> {
        let nodes = &self.files[&file_id];
        let output = nodes
            .iter()
            .map(|(id, mock)| (id.clone(), NodeOutput::from(mock.clone())))
            .collect();
        Warned {
            output: Ok(output),
            warnings: EcoVec::new(),
        }
    }
}

impl ReferenceStateMachine for AbstractState {
    type State = AbstractState;
    type Transition = Transition;

    fn init_state() -> BoxedStrategy<Self::State> {
        Just(AbstractState::new(vec![
            "missing-0".into(),
            "missing-1".into(),
        ]))
        .boxed()
    }

    fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
        let mut strategies: Vec<BoxedStrategy<Transition>> = Vec::new();

        let targets = state.all_targets();
        let pairs = state.all_file_node_pairs();
        let file_ids: Vec<u16> = state.files.keys().copied().collect();

        {
            let next_file_id = state.next_file_id;
            let next_node_id = state.next_node_id;
            let targets = if targets.is_empty() {
                state.missing_ids.clone()
            } else {
                targets.clone()
            };
            strategies.push(
                (1u32..=3)
                    .prop_flat_map(move |count| {
                        let mut node_ids: Vec<String> = Vec::new();
                        for i in 0..count {
                            node_ids.push(format!("n{}", next_node_id + i));
                        }
                        let targets = targets.clone();
                        proptest::collection::vec(mock_node_strategy(targets), count as usize)
                            .prop_map(move |nodes| {
                                let map: HashMap<String, MockNode> =
                                    node_ids.iter().cloned().zip(nodes).collect();
                                Transition::CreateFile {
                                    file_id: next_file_id,
                                    nodes: map,
                                }
                            })
                    })
                    .boxed(),
            );
        }

        if !file_ids.is_empty() {
            let file_ids_clone = file_ids.clone();
            strategies.push(
                proptest::sample::select(file_ids_clone)
                    .prop_map(|file_id| Transition::RemoveFile { file_id })
                    .boxed(),
            );
        }

        if !file_ids.is_empty() {
            let file_ids_clone = file_ids.clone();
            let next_node_id = state.next_node_id;
            let targets = if targets.is_empty() {
                state.missing_ids.clone()
            } else {
                targets.clone()
            };
            strategies.push(
                (
                    proptest::sample::select(file_ids_clone),
                    mock_node_strategy(targets),
                )
                    .prop_map(move |(file_id, node)| Transition::AddNode {
                        file_id,
                        identifier: format!("n{next_node_id}"),
                        node,
                    })
                    .boxed(),
            );
        }

        {
            let removable: Vec<(u16, String)> = state
                .files
                .iter()
                .filter(|(_, nodes)| nodes.len() >= 2)
                .flat_map(|(&fid, nodes)| nodes.keys().map(move |nid| (fid, nid.clone())))
                .collect();
            if !removable.is_empty() {
                strategies.push(
                    proptest::sample::select(removable)
                        .prop_map(|(file_id, identifier)| Transition::RemoveNode {
                            file_id,
                            identifier,
                        })
                        .boxed(),
                );
            }
        }

        if !pairs.is_empty() && !targets.is_empty() {
            let pairs_clone = pairs.clone();
            let targets_clone = targets.clone();
            strategies.push(
                (
                    proptest::sample::select(pairs_clone),
                    proptest::sample::select(targets_clone),
                    metadata_strategy(),
                )
                    .prop_map(|((file_id, node_id), target, metadata)| {
                        Transition::AddTransclusion {
                            file_id,
                            node_id,
                            transclusion: MockTransclusion { target, metadata },
                        }
                    })
                    .boxed(),
            );
        }

        {
            let with_transclusions: Vec<(u16, String, usize)> = state
                .files
                .iter()
                .flat_map(|(&fid, nodes)| {
                    nodes.iter().filter_map(move |(nid, node)| {
                        if node.transclusions.is_empty() {
                            None
                        } else {
                            Some((fid, nid.clone(), node.transclusions.len()))
                        }
                    })
                })
                .collect();
            if !with_transclusions.is_empty() {
                strategies.push(
                    proptest::sample::select(with_transclusions)
                        .prop_flat_map(|(file_id, node_id, len)| {
                            (Just(file_id), Just(node_id), 0..len)
                        })
                        .prop_map(|(file_id, node_id, index)| Transition::RemoveTransclusion {
                            file_id,
                            node_id,
                            index,
                        })
                        .boxed(),
                );
            }
        }

        if !pairs.is_empty() && !targets.is_empty() {
            let pairs_clone = pairs.clone();
            let targets_clone = targets.clone();
            strategies.push(
                (
                    proptest::sample::select(pairs_clone),
                    proptest::sample::select(targets_clone),
                    proptest::option::of("[a-z ]{1,8}"),
                    metadata_strategy(),
                )
                    .prop_map(|((file_id, node_id), target, content, metadata)| {
                        Transition::AddLink {
                            file_id,
                            node_id,
                            link: MockLink {
                                target,
                                content,
                                metadata,
                            },
                        }
                    })
                    .boxed(),
            );
        }

        {
            let with_links: Vec<(u16, String, usize)> = state
                .files
                .iter()
                .flat_map(|(&fid, nodes)| {
                    nodes.iter().filter_map(move |(nid, node)| {
                        if node.links.is_empty() {
                            None
                        } else {
                            Some((fid, nid.clone(), node.links.len()))
                        }
                    })
                })
                .collect();
            if !with_links.is_empty() {
                strategies.push(
                    proptest::sample::select(with_links)
                        .prop_flat_map(|(file_id, node_id, len)| {
                            (Just(file_id), Just(node_id), 0..len)
                        })
                        .prop_map(|(file_id, node_id, index)| Transition::RemoveLink {
                            file_id,
                            node_id,
                            index,
                        })
                        .boxed(),
                );
            }
        }

        if !pairs.is_empty() {
            let pairs_clone = pairs.clone();
            strategies.push(
                (proptest::sample::select(pairs_clone), "[A-Za-z ]{1,12}")
                    .prop_map(|((file_id, node_id), title)| Transition::UpdateTitle {
                        file_id,
                        node_id,
                        title,
                    })
                    .boxed(),
            );
        }

        if !pairs.is_empty() {
            let pairs_clone = pairs.clone();
            strategies.push(
                (proptest::sample::select(pairs_clone), "[a-z ]{0,20}")
                    .prop_map(|((file_id, node_id), body)| Transition::UpdateBody {
                        file_id,
                        node_id,
                        body,
                    })
                    .boxed(),
            );
        }

        if !pairs.is_empty() {
            let pairs_clone = pairs;
            strategies.push(
                (proptest::sample::select(pairs_clone), metadata_strategy())
                    .prop_map(
                        |((file_id, node_id), metadata)| Transition::UpdateMetadata {
                            file_id,
                            node_id,
                            metadata,
                        },
                    )
                    .boxed(),
            );
        }

        proptest::strategy::Union::new(strategies).boxed()
    }

    fn apply(mut state: Self::State, transition: &Self::Transition) -> Self::State {
        match transition {
            Transition::CreateFile { nodes, .. } => {
                state.next_file_id += 1;
                state.next_node_id += nodes.len() as u32;
            }
            Transition::AddNode { .. } => {
                state.next_node_id += 1;
            }
            _ => {}
        }
        state.apply(transition)
    }

    fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
        match transition {
            Transition::CreateFile { file_id, .. } => !state.files.contains_key(file_id),
            Transition::RemoveFile { file_id } => state.files.contains_key(file_id),
            Transition::AddNode {
                file_id,
                identifier,
                ..
            } => {
                state.files.contains_key(file_id) && !state.files[file_id].contains_key(identifier)
            }
            Transition::RemoveNode {
                file_id,
                identifier,
            } => state
                .files
                .get(file_id)
                .is_some_and(|nodes| nodes.len() >= 2 && nodes.contains_key(identifier)),
            Transition::AddTransclusion {
                file_id, node_id, ..
            }
            | Transition::AddLink {
                file_id, node_id, ..
            }
            | Transition::UpdateTitle {
                file_id, node_id, ..
            }
            | Transition::UpdateBody {
                file_id, node_id, ..
            }
            | Transition::UpdateMetadata {
                file_id, node_id, ..
            } => state
                .files
                .get(file_id)
                .is_some_and(|nodes| nodes.contains_key(node_id)),
            Transition::RemoveTransclusion {
                file_id,
                node_id,
                index,
            } => state
                .files
                .get(file_id)
                .and_then(|nodes| nodes.get(node_id))
                .is_some_and(|node| *index < node.transclusions.len()),
            Transition::RemoveLink {
                file_id,
                node_id,
                index,
            } => state
                .files
                .get(file_id)
                .and_then(|nodes| nodes.get(node_id))
                .is_some_and(|node| *index < node.links.len()),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Transition {
    CreateFile {
        file_id: u16,
        nodes: HashMap<String, MockNode>,
    },
    RemoveFile {
        file_id: u16,
    },
    AddNode {
        file_id: u16,
        identifier: String,
        node: MockNode,
    },
    RemoveNode {
        file_id: u16,
        identifier: String,
    },
    AddTransclusion {
        file_id: u16,
        node_id: String,
        transclusion: MockTransclusion,
    },
    RemoveTransclusion {
        file_id: u16,
        node_id: String,
        index: usize,
    },
    AddLink {
        file_id: u16,
        node_id: String,
        link: MockLink,
    },
    RemoveLink {
        file_id: u16,
        node_id: String,
        index: usize,
    },
    UpdateTitle {
        file_id: u16,
        node_id: String,
        title: String,
    },
    UpdateBody {
        file_id: u16,
        node_id: String,
        body: String,
    },
    UpdateMetadata {
        file_id: u16,
        node_id: String,
        metadata: Metadata,
    },
}

impl AbstractState {
    /// Apply a transition to the abstract state, returning the modified state.
    pub fn apply(mut self, transition: &Transition) -> Self {
        match transition {
            Transition::CreateFile { file_id, nodes } => {
                self.files.insert(*file_id, nodes.clone());
            }
            Transition::RemoveFile { file_id } => {
                self.files.remove(file_id);
            }
            Transition::AddNode {
                file_id,
                identifier,
                node,
            } => {
                self.files
                    .get_mut(file_id)
                    .expect("file must exist")
                    .insert(identifier.clone(), node.clone());
            }
            Transition::RemoveNode {
                file_id,
                identifier,
            } => {
                self.files
                    .get_mut(file_id)
                    .expect("file must exist")
                    .remove(identifier);
            }
            Transition::AddTransclusion {
                file_id,
                node_id,
                transclusion,
            } => {
                self.files
                    .get_mut(file_id)
                    .expect("file must exist")
                    .get_mut(node_id)
                    .expect("node must exist")
                    .transclusions
                    .push(transclusion.clone());
            }
            Transition::RemoveTransclusion {
                file_id,
                node_id,
                index,
            } => {
                self.files
                    .get_mut(file_id)
                    .expect("file must exist")
                    .get_mut(node_id)
                    .expect("node must exist")
                    .transclusions
                    .remove(*index);
            }
            Transition::AddLink {
                file_id,
                node_id,
                link,
            } => {
                self.files
                    .get_mut(file_id)
                    .expect("file must exist")
                    .get_mut(node_id)
                    .expect("node must exist")
                    .links
                    .push(link.clone());
            }
            Transition::RemoveLink {
                file_id,
                node_id,
                index,
            } => {
                self.files
                    .get_mut(file_id)
                    .expect("file must exist")
                    .get_mut(node_id)
                    .expect("node must exist")
                    .links
                    .remove(*index);
            }
            Transition::UpdateTitle {
                file_id,
                node_id,
                title,
            } => {
                self.files
                    .get_mut(file_id)
                    .expect("file must exist")
                    .get_mut(node_id)
                    .expect("node must exist")
                    .title = title.clone();
            }
            Transition::UpdateBody {
                file_id,
                node_id,
                body,
            } => {
                self.files
                    .get_mut(file_id)
                    .expect("file must exist")
                    .get_mut(node_id)
                    .expect("node must exist")
                    .body = body.clone();
            }
            Transition::UpdateMetadata {
                file_id,
                node_id,
                metadata,
            } => {
                self.files
                    .get_mut(file_id)
                    .expect("file must exist")
                    .get_mut(node_id)
                    .expect("node must exist")
                    .metadata = metadata.clone();
            }
        }
        self
    }

    /// Returns the file_id affected by this transition (the one that needs recompilation).
    pub fn affected_file_id(&self, transition: &Transition) -> u16 {
        match transition {
            Transition::CreateFile { file_id, .. }
            | Transition::RemoveFile { file_id }
            | Transition::AddNode { file_id, .. }
            | Transition::RemoveNode { file_id, .. }
            | Transition::AddTransclusion { file_id, .. }
            | Transition::RemoveTransclusion { file_id, .. }
            | Transition::AddLink { file_id, .. }
            | Transition::RemoveLink { file_id, .. }
            | Transition::UpdateTitle { file_id, .. }
            | Transition::UpdateBody { file_id, .. }
            | Transition::UpdateMetadata { file_id, .. } => *file_id,
        }
    }
}
