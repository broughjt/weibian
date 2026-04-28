use std::{
    any,
    borrow::Cow,
    collections::{HashMap, HashSet},
    fmt::Write,
    num::NonZeroU16,
    ops::Range,
};

use ecow::EcoVec;
use proptest::{
    arbitrary::any,
    collection::{hash_map, vec},
    option,
    prelude::{BoxedStrategy, Just, Strategy},
    prop_oneof,
    sample::{self, select},
    strategy::Union,
    string::string_regex,
};
use proptest_state_machine::ReferenceStateMachine;
use typst::diag::{SourceDiagnostic, Warned};
use typst_syntax::{FileId, Span};

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
        match transition {
            Transition::CreateFile(CreateFile { file_id, file }) => todo!(),
            Transition::ReplaceFile(replace_file) => todo!(),
            Transition::RemoveFile(remove_file) => todo!(),
            Transition::AddNode(add_node) => todo!(),
            Transition::RemoveNode(remove_node) => todo!(),
            Transition::AddTransclusion(add_transclusion) => todo!(),
            Transition::RemoveTransclusion(remove_transclusion) => todo!(),
            Transition::AddLink(add_link) => todo!(),
            Transition::RemoveLink(remove_link) => todo!(),
            Transition::UpdateTitle(update_title) => todo!(),
            Transition::UpdateBody(update_body) => todo!(),
            Transition::EditMetadata(edit_metadata) => todo!(),
            Transition::UpdateLinkTarget(update_link_target) => todo!(),
            Transition::UpdateLinkContent(update_link_content) => todo!(),
            Transition::UpdateTransclusionTarget(update_transclusion_target) => todo!(),
            Transition::AddCompileError(add_compile_error) => todo!(),
            Transition::RemoveCompileError(remove_compile_error) => todo!(),
            Transition::AddCompileWarning(add_compile_warning) => todo!(),
            Transition::RemoveCompileWarning(remove_compile_warning) => todo!(),
            Transition::RenameNode(rename_node) => todo!(),
        }
    }

    fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
        todo!()
    }
}

// pub type CompiledMockFile = Warned<Result<HashMap<String, NodeOutput>, EcoVec<SourceDiagnostic>>>;

#[derive(Debug, Clone)]
pub struct MockFile {
    pub nodes: HashMap<MockNodeId, MockNode>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct MockNode {
    pub identifier: MockNodeIdentifier,
    pub title: String,
    pub body: String,
    pub span: Span,
    pub metadata: Metadata,
    pub transclusions: Vec<MockTransclusion>,
    pub links: Vec<MockLink>,
}

#[derive(Debug, Clone)]
pub struct MockTransclusion {
    pub target: MockNodeIdentifier,
    pub metadata: Metadata,
}

#[derive(Debug, Clone)]
pub struct MockLink {
    pub target: MockNodeIdentifier,
    pub content: Option<String>,
    pub metadata: Metadata,
}

impl From<MockNode> for NodeOutput {
    fn from(node: MockNode) -> Self {
        let mut body_html = String::new();
        let mut transclusion_metadata: HashMap<u32, Metadata> = HashMap::new();
        let mut link_metadata: HashMap<u32, Metadata> = HashMap::new();
        let mut transclusions = Vec::with_capacity(node.transclusions.len());
        let mut links = Vec::with_capacity(node.links.len());

        if !node.body.is_empty() {
            write!(body_html, "<p>{}</p>", node.body).unwrap();
        }

        for (counter, transclusion) in node.transclusions.into_iter().enumerate() {
            let counter = counter as u32;
            write!(
                body_html,
                r#"<wb-transclude identifier="{}" counter="{counter}"></wb-transclude>"#,
                transclusion.target.0,
            )
            .unwrap();
            if !transclusion.metadata.is_empty() {
                transclusion_metadata.insert(counter, transclusion.metadata);
            }
            transclusions.push(transclusion.target.0.to_string());
        }

        for (counter, link) in node.links.into_iter().enumerate() {
            let counter = counter as u32;
            let content = link.content.as_deref().unwrap_or_default();
            write!(
                body_html,
                r#"<a href="wb:{}" data-counter="{counter}">{content}</a>"#,
                link.target.0,
            )
            .unwrap();
            if !link.metadata.is_empty() {
                link_metadata.insert(counter, link.metadata);
            }
            links.push(link.target.0.to_string());
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

// We have `MockNodeId` and `MockNodeIdentifier` separately because we want to
// allow duplicate ids. If we key by identifiers like the compiler does, our
// data model can't express duplicates.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MockNodeId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MockNodeIdentifier(pub u32);

#[derive(Clone, Debug, Default)]
pub struct FileState {
    pub nodes: HashSet<MockNodeId>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct State {
    pub files: HashMap<NonZeroU16, FileState>,
    pub nodes: HashMap<MockNodeId, MockNode>,
}

impl State {
    fn queries(&self) -> Queries {
        // let new_file_id = self
        //     .files
        //     .keys()
        //     .max()
        //     .map_or(NonZeroU16::new(1), |k| k.checked_add(1))
        //     .unwrap();
        // let existing_file_ids: Cow<'static, [NonZeroU16]> = self.files.keys().copied().collect();

        // Queries {
        //     new_file_id,
        //     existing_file_ids,
        // }

        todo!()
    }

    fn create_file_strategy(&self, queries: &Queries) -> impl Strategy<Value = CreateFile> + use<> {
        let file_id = queries.next_file_id;

        // TODO:
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

    // TODO: Is it worth trying to generate missing removes?
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

    fn add_node_strategy(
        &self,
        queries: &Queries,
    ) -> Option<impl Strategy<Value = AddNode> + use<>> {
        let Queries {
            existing_file_ids,
            next_node_id,
            ..
        } = queries;

        if !queries.existing_file_ids.is_empty() {
            Some(
                select(queries.existing_file_ids.clone()).prop_map(|file_id| AddNode {
                    file_id,
                    node_id: *next_node_id,
                    node: todo!(),
                }),
            )
        } else {
            None
        }
    }

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
    next_file_id: NonZeroU16,
    existing_file_ids: Cow<'static, [NonZeroU16]>,
    next_node_id: MockNodeId,
    existing_node_identifiers: Cow<'static, [MockNodeId]>,
    missing_node_identifiers: Cow<'static, [MockNodeId]>,
    next_missing_node_identifier: MockNodeId,
}

const METADATA_ENTRIES_MAX: usize = 6;
const METADATA_VALUES_MAX: usize = 4;
const NODE_TRANSCLUSIONS_MAX: usize = 5;
const NODE_LINKS_MAX: usize = 5;

fn mock_file_strategy() -> impl Strategy<Value = MockFile> {
    todo!()
}

fn mock_file_node_strategy(
    file_id: NonZeroU16,
    existing: Cow<'static, [String]>,
    missing: Cow<'static, [String]>,
    new: String,
) -> impl Strategy<Value = MockFileNode> {
    (
        node_identifier_strategy(existing.clone(), missing),
        mock_node_strategy(file_id, existing),
    )
        .prop_map(|(identifier, node)| MockFileNode { identifier, node })
}

fn mock_node_strategy(
    file_id: NonZeroU16,
    existing: Cow<'static, [MockNodeIdentifier]>,
    missing: Cow<'static, [MockNodeIdentifier]>,
    next_missing: MockNodeIdentifier,
    next: MockNodeIdentifier,
) -> impl Strategy<Value = MockNode> {
    (
        node_identifier_strategy(existing, missing, next),
        title_strategy(),
        body_strategy(),
        span_strategy(file_id),
        metadata_strategy(),
        vec(
            mock_transclusion_strategy(existing.clone(), missing.clone(), next_missing),
            0..NODE_TRANSCLUSIONS_MAX,
        ),
        vec(
            mock_link_strategy(existing, missing, next_missing),
            0..NODE_LINKS_MAX,
        ),
    )
        .prop_map(
            |(identifier, title, body, span, metadata, transclusions, links)| MockNode {
                identifier,
                title,
                body,
                span,
                metadata,
                transclusions,
                links,
            },
        )
}

fn span_strategy(file_id: NonZeroU16) -> impl Strategy<Value = Span> {
    range_strategy().prop_map(move |range| Span::from_range(FileId::from_raw(file_id), range))
}

fn metadata_strategy() -> impl Strategy<Value = Metadata> {
    hash_map(
        metadata_key_strategy(),
        vec(metadata_key_strategy(), 0..=METADATA_VALUES_MAX),
        0..=METADATA_ENTRIES_MAX,
    )
}

fn mock_transclusion_strategy(
    existing: Cow<'static, [MockNodeIdentifier]>,
    missing: Cow<'static, [MockNodeIdentifier]>,
    next_missing: MockNodeIdentifier,
) -> impl Strategy<Value = MockTransclusion> {
    (
        target_strategy(existing, missing, next_missing),
        metadata_strategy(),
    )
        .prop_map(|(target, metadata)| MockTransclusion { target, metadata })
}

fn mock_link_strategy(
    existing: Cow<'static, [MockNodeIdentifier]>,
    missing: Cow<'static, [MockNodeIdentifier]>,
    next_missing: MockNodeIdentifier,
) -> impl Strategy<Value = MockLink> {
    (
        target_strategy(existing, missing, next_missing),
        option::of(link_content_strategy()),
        metadata_strategy(),
    )
        .prop_map(|(target, content, metadata)| MockLink {
            target,
            content,
            metadata,
        })
}

fn node_identifier_strategy(
    existing: Cow<'static, [MockNodeIdentifier]>,
    missing: Cow<'static, [MockNodeIdentifier]>,
    next: MockNodeIdentifier,
) -> impl Strategy<Value = MockNodeIdentifier> {
    prop_oneof![select(existing), select(missing), Just(next)]
}

fn target_strategy(
    existing: Cow<'static, [MockNodeIdentifier]>,
    missing: Cow<'static, [MockNodeIdentifier]>,
    next_missing: MockNodeIdentifier,
) -> impl Strategy<Value = MockNodeIdentifier> {
    prop_oneof![select(existing), select(missing), Just(next_missing)]
}

// Helpers

fn body_strategy() -> impl Strategy<Value = String> {
    "[a-z]*"
}

fn title_strategy() -> impl Strategy<Value = String> {
    "[a-z]*"
}

fn identifier_strategy() -> impl Strategy<Value = String> {
    "[a-z]{0,5}"
}

fn metadata_key_strategy() -> impl Strategy<Value = String> {
    "[a-z]*"
}

fn link_content_strategy() -> impl Strategy<Value = String> {
    "[a-z]+"
}

fn range_strategy() -> impl Strategy<Value = Range<usize>> {
    any::<(usize, usize)>().prop_map(|(a, b)| {
        let start = a.min(b);
        let end = a.max(b);
        start..end
    })
}
