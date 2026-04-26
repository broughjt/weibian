use std::{borrow::Cow, collections::HashMap, fmt::Write, num::NonZeroU16};

use ecow::EcoVec;
use proptest::{
    prelude::{BoxedStrategy, Strategy},
    sample::select,
    strategy::Union,
};
use proptest_state_machine::ReferenceStateMachine;
use typst::diag::{SourceDiagnostic, Warned};
use typst_syntax::Span;

use crate::compiler::{Metadata, NodeEntry, extract::NodeOutput};

pub struct ReferenceCompiler;

impl ReferenceStateMachine for ReferenceCompiler {
    type State = State;
    type Transition = Transition;

    fn init_state() -> BoxedStrategy<State> {
        todo!()
    }

    fn transitions(state: &State) -> BoxedStrategy<Transition> {
        const CREATE_FILE_WEIGHT: u32 = 2;
        const REPLACE_FILE_WEIGHT: u32 = 1;
        const REMOVE_FILE_WEIGHT: u32 = 1;

        let mut strategies: Vec<(u32, BoxedStrategy<Transition>)> = Vec::new();

        let queries = state.queries();

        strategies.push((
            CREATE_FILE_WEIGHT,
            state
                .create_file_strategy(&queries)
                .prop_map(Transition::CreateFile)
                .boxed(),
        ));

        if let Some(replace_file) = state.replace_file_strategy(&queries) {
            strategies.push((
                REPLACE_FILE_WEIGHT,
                replace_file.prop_map(Transition::ReplaceFile).boxed(),
            ));
        }

        if let Some(remove_file) = state.remove_file_strategy(&queries) {
            strategies.push((
                REMOVE_FILE_WEIGHT,
                remove_file.prop_map(Transition::RemoveFile).boxed(),
            ))
        }

        Union::new_weighted(strategies).boxed()
    }

    fn apply(state: State, transition: &Transition) -> State {
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
pub struct MockNodeId(pub u32);

#[derive(Debug, Clone)]
pub struct MockFileNode {
    pub identifier: String,
    pub node: MockNode,
}

#[derive(Clone, Debug, Default)]
pub struct MockFile {
    pub nodes: HashMap<MockNodeId, MockFileNode>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct State {
    pub files: HashMap<NonZeroU16, MockFile>,
}

impl State {
    fn queries(&self) -> Queries {
        let new_file_id = self
            .files
            .keys()
            .max()
            .map_or(NonZeroU16::new(1), |k| k.checked_add(1))
            .unwrap();
        let existing_file_ids: Cow<'static, [NonZeroU16]> = self.files.keys().copied().collect();

        Queries {
            new_file_id,
            existing_file_ids,
        }
    }

    fn create_file_strategy(&self, queries: &Queries) -> impl Strategy<Value = CreateFile> + use<> {
        let file_id = queries.new_file_id;

        mock_file_strategy().prop_map(move |file| CreateFile { file_id, file })
    }

    fn replace_file_strategy(
        &self,
        queries: &Queries,
    ) -> Option<impl Strategy<Value = ReplaceFile> + use<>> {
        if !queries.existing_file_ids.is_empty() {
            Some(
                (
                    select(queries.existing_file_ids.clone()),
                    mock_file_strategy(),
                )
                    .prop_map(move |(file_id, file)| ReplaceFile { file_id, file }),
            )
        } else {
            None
        }
    }

    fn remove_file_strategy(
        &self,
        queries: &Queries,
    ) -> Option<impl Strategy<Value = RemoveFile> + use<>> {
        if !queries.existing_file_ids.is_empty() {
            Some(
                select(queries.existing_file_ids.clone())
                    .prop_map(|file_id| RemoveFile { file_id }),
            )
        } else {
            None
        }
    }

    // fn add_node_strategy(
    //     &self,
    //     queries: &Queries,
    // ) -> Option<impl Strategy<Value = AddNode> + use<>> {
    //     todo!()
    // }

    // fn remove_node_strategy(
    //     &self,
    //     queries: &Queries,
    // ) -> Option<impl Strategy<Value = RemoveNode> + use<>> {
    //     todo!()
    // }

    // fn add_transclusion_strategy(
    //     &self,
    //     queries: &Queries,
    // ) -> Option<impl Strategy<Value = AddTransclusion> + use<>> {
    //     todo!()
    // }

    // fn remove_transclusion_strategy(
    //     &self,
    //     queries: &Queries,
    // ) -> Option<impl Strategy<Value = RemoveTransclusion> + use<>> {
    //     todo!()
    // }

    // fn add_link_strategy(
    //     &self,
    //     queries: &Queries,
    // ) -> Option<impl Strategy<Value = AddLink> + use<>> {
    //     todo!()
    // }

    // fn remove_link_strategy(
    //     &self,
    //     queries: &Queries,
    // ) -> Option<impl Strategy<Value = RemoveLink> + use<>> {
    //     todo!()
    // }

    // fn update_title_strategy(
    //     &self,
    //     queries: &Queries,
    // ) -> Option<impl Strategy<Value = UpdateTitle> + use<>> {
    //     todo!()
    // }

    // fn update_body_strategy(
    //     &self,
    //     queries: &Queries,
    // ) -> Option<impl Strategy<Value = UpdateBody> + use<>> {
    //     todo!()
    // }

    // TODO:
    // fn edit_metadata_strategy(
    //     &self,
    //     queries: &Queries,
    // ) -> Option<impl Strategy<Value = EditMetadata> + use<>> {
    //     todo!()
    // }

    // fn update_link_target_strategy(
    //     &self,
    //     queries: &Queries,
    // ) -> Option<impl Strategy<Value = UpdateLinkTarget> + use<>> {
    //     todo!()
    // }

    // fn update_link_content_strategy(
    //     &self,
    //     queries: &Queries,
    // ) -> Option<impl Strategy<Value = UpdateLinkContent> + use<>> {
    //     todo!()
    // }

    // fn update_transclusion_target_strategy(
    //     &self,
    //     queries: &Queries,
    // ) -> Option<impl Strategy<Value = UpdateTransclusionTarget> + use<>> {
    //     todo!()
    // }

    // fn add_compile_error_strategy(
    //     &self,
    //     queries: &Queries,
    // ) -> Option<impl Strategy<Value = AddCompileError> + use<>> {
    //     todo!()
    // }

    // fn remove_compile_error_strategy(
    //     &self,
    //     queries: &Queries,
    // ) -> Option<impl Strategy<Value = RemoveCompileError> + use<>> {
    //     todo!()
    // }

    // fn add_compile_warning_strategy(
    //     &self,
    //     queries: &Queries,
    // ) -> Option<impl Strategy<Value = AddCompileWarning> + use<>> {
    //     todo!()
    // }

    // fn remove_compile_warning_strategy(
    //     &self,
    //     queries: &Queries,
    // ) -> Option<impl Strategy<Value = RemoveCompileWarning> + use<>> {
    //     todo!()
    // }

    // fn rename_node_strategy(
    //     &self,
    //     queries: &Queries,
    // ) -> Option<impl Strategy<Value = RenameNode> + use<>> {
    //     todo!()
    // }
}

#[derive(Debug, Clone)]
pub enum Transition {
    CreateFile(CreateFile),
    ReplaceFile(ReplaceFile),
    RemoveFile(RemoveFile),
    AddNode(AddNode),
    RemoveNode(RemoveNode),
    AddTransclusion(AddTransclusion),
    RemoveTransclusion(RemoveTransclusion),
    AddLink(AddLink),
    RemoveLink(RemoveLink),
    UpdateTitle(UpdateTitle),
    UpdateBody(UpdateBody),
    EditMetadata(EditMetadata),
    UpdateLinkTarget(UpdateLinkTarget),
    UpdateLinkContent(UpdateLinkContent),
    UpdateTransclusionTarget(UpdateTransclusionTarget),
    AddCompileError(AddCompileError),
    RemoveCompileError(RemoveCompileError),
    AddCompileWarning(AddCompileWarning),
    RemoveCompileWarning(RemoveCompileWarning),
    RenameNode(RenameNode),
}

#[derive(Debug, Clone)]
pub struct CreateFile {
    pub file_id: NonZeroU16,
    pub file: MockFile,
}

#[derive(Debug, Clone)]
pub struct ReplaceFile {
    pub file_id: NonZeroU16,
    pub file: MockFile,
}

#[derive(Debug, Clone)]
pub struct RemoveFile {
    pub file_id: NonZeroU16,
}

#[derive(Debug, Clone)]
pub struct AddNode {
    pub file_id: NonZeroU16,
    pub node_id: MockNodeId,
    pub node: MockFileNode,
}

#[derive(Debug, Clone)]
pub struct RemoveNode {
    pub file_id: NonZeroU16,
    pub node_id: MockNodeId,
}

#[derive(Debug, Clone)]
pub struct AddTransclusion {
    pub file_id: NonZeroU16,
    pub node_id: MockNodeId,
    pub transclusion: MockTransclusion,
}

#[derive(Debug, Clone)]
pub struct RemoveTransclusion {
    pub file_id: NonZeroU16,
    pub node_id: MockNodeId,
    pub index: u32,
}

#[derive(Debug, Clone)]
pub struct AddLink {
    pub file_id: NonZeroU16,
    pub node_id: MockNodeId,
    pub link: MockLink,
}

#[derive(Debug, Clone)]
pub struct RemoveLink {
    pub file_id: NonZeroU16,
    pub node_id: MockNodeId,
    pub index: u32,
}

#[derive(Debug, Clone)]
pub struct UpdateTitle {
    pub file_id: NonZeroU16,
    pub node_id: MockNodeId,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct UpdateBody {
    pub file_id: NonZeroU16,
    pub node_id: MockNodeId,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct EditMetadata {
    pub file_id: NonZeroU16,
    pub node_id: MockNodeId,
    pub target: MetadataTarget,
    pub operation: MetadataOperation,
}

#[derive(Debug, Clone)]
pub struct UpdateLinkTarget {
    pub file_id: NonZeroU16,
    pub node_id: MockNodeId,
    pub link_index: u32,
    pub new_target: String,
}

#[derive(Debug, Clone)]
pub struct UpdateLinkContent {
    pub file_id: NonZeroU16,
    pub node_id: MockNodeId,
    pub link_index: u32,
    pub new_content: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UpdateTransclusionTarget {
    pub file_id: NonZeroU16,
    pub node_id: MockNodeId,
    pub transclusion_index: u32,
    pub new_target: String,
}

#[derive(Debug, Clone)]
pub struct AddCompileError {
    pub file_id: NonZeroU16,
    pub error: String,
}

#[derive(Debug, Clone)]
pub struct RemoveCompileError {
    pub file_id: NonZeroU16,
    pub index: usize,
}

#[derive(Debug, Clone)]
pub struct AddCompileWarning {
    pub file_id: NonZeroU16,
    pub warning: String,
}

#[derive(Debug, Clone)]
pub struct RemoveCompileWarning {
    pub file_id: NonZeroU16,
    pub index: usize,
}

#[derive(Debug, Clone)]
pub struct RenameNode {
    pub file_id: NonZeroU16,
    pub node_id: MockNodeId,
    pub new_id: String,
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

struct Queries {
    new_file_id: NonZeroU16,
    existing_file_ids: Cow<'static, [NonZeroU16]>,
}

fn mock_file_strategy() -> BoxedStrategy<MockFile> {
    todo!()
}
