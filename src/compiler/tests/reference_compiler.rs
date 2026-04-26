use std::{collections::HashMap, fmt::Write, num::NonZeroU16};

use ecow::EcoVec;
use proptest::prelude::BoxedStrategy;
use proptest_state_machine::ReferenceStateMachine;
use typst::diag::{SourceDiagnostic, Warned};
use typst_syntax::Span;

use crate::compiler::{Metadata, NodeEntry, extract::NodeOutput};

pub struct ReferenceCompiler;

impl ReferenceStateMachine for ReferenceCompiler {
    type State = State;
    type Transition = Transition;

    fn init_state() -> BoxedStrategy<Self::State> {
        todo!()
    }

    fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
        todo!()
    }

    fn apply(state: Self::State, transition: &Self::Transition) -> Self::State {
        todo!()
    }
}

pub type CompiledMockFile = Warned<Result<HashMap<String, NodeOutput>, EcoVec<SourceDiagnostic>>>;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MockNodeHandle(pub u32);

#[derive(Debug, Clone)]
pub struct MockFileNode {
    pub identifier: String,
    pub node: MockNode,
}

#[derive(Clone, Debug, Default)]
pub struct MockFile {
    pub nodes: HashMap<MockNodeHandle, MockFileNode>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct State {
    pub files: HashMap<NonZeroU16, MockFile>,
}

#[derive(Debug, Clone)]
pub enum MetadataTarget {
    Node,
    Link { index: u32 },
    Transclusion { index: u32 },
}

#[derive(Debug, Clone)]
pub enum MetadataOperation {
    ReplaceAll(Metadata),
    InsertKey {
        key: String,
        values: Vec<String>,
    },
    RemoveKey {
        key: String,
    },
    AppendValue {
        key: String,
        value: String,
    },
    RemoveValue {
        key: String,
        index: usize,
    },
    ReplaceValue {
        key: String,
        index: usize,
        new_value: String,
    },
    Clear,
}

#[derive(Debug, Clone)]
pub enum Transition {
    CreateFile {
        file_id: u16,
        file: MockFile,
    },
    ReplaceFile {
        file_id: u16,
        file: MockFile,
    },
    RemoveFile {
        file_id: u16,
    },
    AddNode {
        file_id: u16,
        node: MockFileNode,
    },
    RemoveNode {
        file_id: u16,
        node: MockNodeHandle,
    },
    AddTransclusion {
        file_id: u16,
        node: MockNodeHandle,
        transclusion: MockTransclusion,
    },
    RemoveTransclusion {
        file_id: u16,
        node: MockNodeHandle,
        index: u32,
    },
    AddLink {
        file_id: u16,
        node: MockNodeHandle,
        link: MockLink,
    },
    RemoveLink {
        file_id: u16,
        node: MockNodeHandle,
        index: u32,
    },
    UpdateTitle {
        file_id: u16,
        node: MockNodeHandle,
        title: String,
    },
    UpdateBody {
        file_id: u16,
        node: MockNodeHandle,
        body: String,
    },
    EditMetadata {
        file_id: u16,
        node: MockNodeHandle,
        target: MetadataTarget,
        operation: MetadataOperation,
    },
    UpdateLinkTarget {
        file_id: u16,
        node: MockNodeHandle,
        link_index: u32,
        new_target: String,
    },
    UpdateLinkContent {
        file_id: u16,
        node: MockNodeHandle,
        link_index: u32,
        new_content: Option<String>,
    },
    UpdateTransclusionTarget {
        file_id: u16,
        node: MockNodeHandle,
        transclusion_index: u32,
        new_target: String,
    },
    AddCompileError {
        file_id: u16,
        error: String,
    },
    RemoveCompileError {
        file_id: u16,
        index: usize,
    },
    AddCompileWarning {
        file_id: u16,
        warning: String,
    },
    RemoveCompileWarning {
        file_id: u16,
        index: usize,
    },
    RenameNode {
        file_id: u16,
        node: MockNodeHandle,
        new_id: String,
    },
}
