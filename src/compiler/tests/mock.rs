use std::collections::HashMap;
use std::fmt::Write;
use std::num::NonZeroU16;

use ecow::EcoVec;
use typst::diag::{SourceDiagnostic, Warned};
use typst::syntax::{FileId, Span};

use crate::compiler::Metadata;
use crate::compiler::NodeEntry;
use crate::compiler::extract::NodeOutput;

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
