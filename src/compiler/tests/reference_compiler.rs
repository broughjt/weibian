use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    num::NonZeroU16,
    ops::Range,
};

use ecow::{EcoVec, eco_format};
use proptest::{
    collection::{hash_map, vec},
    option,
    prelude::{BoxedStrategy, Just, Strategy, any},
    prop_assert, proptest,
    sample::select,
    strategy::Union,
};
use proptest_state_machine::ReferenceStateMachine;
use typst::diag::{SourceDiagnostic, Warned};
use typst_syntax::{FileId, Span};

use crate::compiler::{Metadata, extract::NodeOutput, tests::model::*};

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
        const ADD_NODE_WEIGHT: u32 = 1;
        const REMOVE_NODE_WEIGHT: u32 = 1;
        const ADD_TRANSCLUSION_WEIGHT: u32 = 1;
        const REMOVE_TRANSCLUSION_WEIGHT: u32 = 1;
        const ADD_LINK_WEIGHT: u32 = 1;
        const REMOVE_LINK_WEIGHT: u32 = 1;
        const UPDATE_TITLE_WEIGHT: u32 = 1;
        const UPDATE_BODY_WEIGHT: u32 = 1;
        const UPDATE_LINK_TARGET_WEIGHT: u32 = 1;
        const UPDATE_LINK_CONTENT_WEIGHT: u32 = 1;
        const UPDATE_TRANSCLUSION_TARGET_WEIGHT: u32 = 1;

        let mut strategies: Vec<(u32, BoxedStrategy<Transition>)> = Vec::new();

        let queries = state.queries();

        if let Some(create_file) = CreateFile::strategy(state, &queries) {
            strategies.push((
                CREATE_FILE_WEIGHT,
                create_file.prop_map(Transition::CreateFile).boxed(),
            ));
        }
        if let Some(replace_file) = ReplaceFile::strategy(state, &queries) {
            strategies.push((
                REPLACE_FILE_WEIGHT,
                replace_file.prop_map(Transition::ReplaceFile).boxed(),
            ));
        }
        if let Some(remove_file) = RemoveFile::strategy(state, &queries) {
            strategies.push((
                REMOVE_FILE_WEIGHT,
                remove_file.prop_map(Transition::RemoveFile).boxed(),
            ));
        }
        if let Some(add_node) = AddNode::strategy(state, &queries) {
            strategies.push((
                ADD_NODE_WEIGHT,
                add_node.prop_map(Transition::AddNode).boxed(),
            ));
        }
        if let Some(remove_node) = RemoveNode::strategy(state, &queries) {
            strategies.push((
                REMOVE_NODE_WEIGHT,
                remove_node.prop_map(Transition::RemoveNode).boxed(),
            ));
        }
        if let Some(add_transclusion) = AddTransclusion::strategy(state, &queries) {
            strategies.push((
                ADD_TRANSCLUSION_WEIGHT,
                add_transclusion
                    .prop_map(Transition::AddTransclusion)
                    .boxed(),
            ));
        }
        if let Some(remove_transclusion) = RemoveTransclusion::strategy(state, &queries) {
            strategies.push((
                REMOVE_TRANSCLUSION_WEIGHT,
                remove_transclusion
                    .prop_map(Transition::RemoveTransclusion)
                    .boxed(),
            ));
        }
        if let Some(add_link) = AddLink::strategy(state, &queries) {
            strategies.push((
                ADD_LINK_WEIGHT,
                add_link.prop_map(Transition::AddLink).boxed(),
            ));
        }
        if let Some(remove_link) = RemoveLink::strategy(state, &queries) {
            strategies.push((
                REMOVE_LINK_WEIGHT,
                remove_link.prop_map(Transition::RemoveLink).boxed(),
            ));
        }
        if let Some(update_title) = UpdateTitle::strategy(state, &queries) {
            strategies.push((
                UPDATE_TITLE_WEIGHT,
                update_title.prop_map(Transition::UpdateTitle).boxed(),
            ));
        }
        if let Some(update_body) = UpdateBody::strategy(state, &queries) {
            strategies.push((
                UPDATE_BODY_WEIGHT,
                update_body.prop_map(Transition::UpdateBody).boxed(),
            ));
        }
        if let Some(update_link_target) = UpdateLinkTarget::strategy(state, &queries) {
            strategies.push((
                UPDATE_LINK_TARGET_WEIGHT,
                update_link_target
                    .prop_map(Transition::UpdateLinkTarget)
                    .boxed(),
            ));
        }
        if let Some(update_link_content) = UpdateLinkContent::strategy(state, &queries) {
            strategies.push((
                UPDATE_LINK_CONTENT_WEIGHT,
                update_link_content
                    .prop_map(Transition::UpdateLinkContent)
                    .boxed(),
            ));
        }
        if let Some(update_transclusion_target) =
            UpdateTransclusionTarget::strategy(state, &queries)
        {
            strategies.push((
                UPDATE_TRANSCLUSION_TARGET_WEIGHT,
                update_transclusion_target
                    .prop_map(Transition::UpdateTransclusionTarget)
                    .boxed(),
            ));
        }

        Union::new_weighted(strategies).boxed()
    }

    fn apply(state: State, transition: &Transition) -> State {
        match transition {
            Transition::CreateFile(create_file) => create_file.apply(state),
            Transition::ReplaceFile(replace_file) => replace_file.apply(state),
            Transition::RemoveFile(remove_file) => remove_file.apply(state),
            Transition::AddNode(add_node) => add_node.apply(state),
            Transition::RemoveNode(remove_node) => remove_node.apply(state),
            Transition::AddTransclusion(add_transclusion) => add_transclusion.apply(state),
            Transition::RemoveTransclusion(remove_transclusion) => remove_transclusion.apply(state),
            Transition::AddLink(add_link) => add_link.apply(state),
            Transition::RemoveLink(remove_link) => remove_link.apply(state),
            Transition::UpdateTitle(update_title) => update_title.apply(state),
            Transition::UpdateBody(update_body) => update_body.apply(state),
            Transition::EditMetadata(edit_metadata) => todo!(),
            Transition::UpdateLinkTarget(update_link_target) => update_link_target.apply(state),
            Transition::UpdateLinkContent(update_link_content) => update_link_content.apply(state),
            Transition::UpdateTransclusionTarget(update_transclusion_target) => {
                update_transclusion_target.apply(state)
            }
            Transition::AddCompileError(add_compile_error) => todo!(),
            Transition::RemoveCompileError(remove_compile_error) => todo!(),
            Transition::AddCompileWarning(add_compile_warning) => todo!(),
            Transition::RemoveCompileWarning(remove_compile_warning) => todo!(),
            Transition::RenameNode(rename_node) => todo!(),
        }
    }

    fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
        match transition {
            Transition::CreateFile(create_file) => create_file.does_apply(state),
            Transition::ReplaceFile(replace_file) => replace_file.does_apply(state),
            Transition::RemoveFile(remove_file) => remove_file.does_apply(state),
            Transition::AddNode(add_node) => add_node.does_apply(state),
            Transition::RemoveNode(remove_node) => remove_node.does_apply(state),
            Transition::AddTransclusion(add_transclusion) => add_transclusion.does_apply(state),
            Transition::RemoveTransclusion(remove_transclusion) => {
                remove_transclusion.does_apply(state)
            }
            Transition::AddLink(add_link) => add_link.does_apply(state),
            Transition::RemoveLink(remove_link) => remove_link.does_apply(state),
            Transition::UpdateTitle(update_title) => update_title.does_apply(state),
            Transition::UpdateBody(update_body) => update_body.does_apply(state),
            Transition::EditMetadata(edit_metadata) => todo!(),
            Transition::UpdateLinkTarget(update_link_target) => {
                update_link_target.does_apply(state)
            }
            Transition::UpdateLinkContent(update_link_content) => {
                update_link_content.does_apply(state)
            }
            Transition::UpdateTransclusionTarget(update_transclusion_target) => {
                update_transclusion_target.does_apply(state)
            }
            Transition::AddCompileError(add_compile_error) => todo!(),
            Transition::RemoveCompileError(remove_compile_error) => todo!(),
            Transition::AddCompileWarning(add_compile_warning) => todo!(),
            Transition::RemoveCompileWarning(remove_compile_warning) => todo!(),
            Transition::RenameNode(rename_node) => todo!(),
        }
    }
}

// We have both `MockNodeId` and `MockNodeIdentifier` separately because we want
// to allow duplicate ids. If we key by identifiers like the compiler does, our
// data model can't express duplicates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MockNodeId(pub u32);

#[derive(Debug, Clone)]
pub struct MockFile {
    pub nodes: HashMap<MockNodeId, MockNode>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct FileState {
    pub nodes: HashSet<MockNodeId>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

// TODO: Invariant — every `MockNodeId` in `nodes` is owned by exactly one
// `FileState` (i.e. appears in exactly one `files[*].nodes`). All
// `Event::apply` impls must maintain this. Worth writing a property test that
// asserts the invariant after each transition.
#[derive(Clone, Debug)]
pub struct State {
    pub files: HashMap<FileId, FileState>,
    pub nodes: HashMap<MockNodeId, MockNode>,
}

impl State {
    /// Builds the `_update` payload that the incremental compiler would
    /// receive after compiling this file: errors and warnings translated
    /// into [`SourceDiagnostic`]s, and (when there are no errors) all the
    /// file's nodes converted into [`NodeOutput`]s keyed by identifier.
    pub fn compile_file(
        &self,
        file_id: FileId,
    ) -> Warned<Result<HashMap<String, NodeOutput>, EcoVec<SourceDiagnostic>>> {
        let file = &self.files[&file_id];

        let warnings: EcoVec<SourceDiagnostic> = file
            .warnings
            .iter()
            .map(|msg| SourceDiagnostic::warning(Span::detached(), eco_format!("{msg}")))
            .collect();
        let errors: EcoVec<SourceDiagnostic> = file
            .errors
            .iter()
            .map(|msg| SourceDiagnostic::error(Span::detached(), eco_format!("{msg}")))
            .collect();

        let output = if !errors.is_empty() {
            Err(errors)
        } else {
            let nodes: HashMap<String, NodeOutput> = file
                .nodes
                .iter()
                .map(|node_id| {
                    let mock = &self.nodes[node_id];
                    (
                        mock.identifier.0.to_string(),
                        NodeOutput::from(mock.clone()),
                    )
                })
                .collect();
            Ok(nodes)
        };

        Warned { output, warnings }
    }

    fn remove_file_nodes(&mut self, file_id: FileId) {
        let file = self
            .files
            .get(&file_id)
            .expect("bug: file must exist before its nodes can be removed");

        for node_id in &file.nodes {
            let removed = self.nodes.remove(node_id);
            assert!(
                removed.is_some(),
                "bug: file-owned node missing from global node store"
            );
        }
    }

    fn queries(&self) -> Queries {
        let next_file_id = FileId::from_raw(
            self.files
                .keys()
                .map(|id| id.into_raw())
                .max()
                .map_or(NonZeroU16::new(1).unwrap(), |raw| {
                    raw.checked_add(1).expect("file id overflow")
                }),
        );

        let mut existing_file_ids: Vec<FileId> = self.files.keys().copied().collect();
        existing_file_ids.sort_by_key(|id| id.into_raw());

        let mut existing_file_node_pairs: Vec<(FileId, MockNodeId)> = self
            .files
            .iter()
            .flat_map(|(&file_id, file)| {
                file.nodes
                    .iter()
                    .copied()
                    .map(move |node_id| (file_id, node_id))
            })
            .collect();
        existing_file_node_pairs.sort_by_key(|(file_id, node_id)| (file_id.into_raw(), node_id.0));

        let mut existing_file_node_transclusion_triples: Vec<(FileId, MockNodeId, u32)> = self
            .files
            .iter()
            .flat_map(|(&file_id, file)| {
                file.nodes.iter().copied().flat_map(move |node_id| {
                    let transclusion_len = self
                        .nodes
                        .get(&node_id)
                        .expect("bug: file-owned node missing from global node store")
                        .transclusions
                        .len() as u32;
                    (0..transclusion_len).map(move |index| (file_id, node_id, index))
                })
            })
            .collect();
        existing_file_node_transclusion_triples
            .sort_by_key(|(file_id, node_id, index)| (file_id.into_raw(), node_id.0, *index));

        let mut existing_file_node_link_triples: Vec<(FileId, MockNodeId, u32)> = self
            .files
            .iter()
            .flat_map(|(&file_id, file)| {
                file.nodes.iter().copied().flat_map(move |node_id| {
                    let link_len = self
                        .nodes
                        .get(&node_id)
                        .expect("bug: file-owned node missing from global node store")
                        .links
                        .len() as u32;
                    (0..link_len).map(move |index| (file_id, node_id, index))
                })
            })
            .collect();
        existing_file_node_link_triples
            .sort_by_key(|(file_id, node_id, index)| (file_id.into_raw(), node_id.0, *index));

        let next_node_id = MockNodeId(
            self.nodes
                .keys()
                .map(|MockNodeId(n)| *n)
                .max()
                .map_or(0, |n| n + 1),
        );

        let existing_identifiers: HashSet<MockNodeIdentifier> =
            self.nodes.values().map(|n| n.identifier).collect();

        let mut missing_identifiers: HashSet<MockNodeIdentifier> = HashSet::new();
        for node in self.nodes.values() {
            for t in &node.transclusions {
                if !existing_identifiers.contains(&t.target) {
                    missing_identifiers.insert(t.target);
                }
            }
            for l in &node.links {
                if !existing_identifiers.contains(&l.target) {
                    missing_identifiers.insert(l.target);
                }
            }
        }

        // `next_node_identifier` and `next_missing_node_identifier` are both
        // fresh (greater than any identifier we've seen as a node or as a
        // dangling target), and they are deliberately distinct so that a
        // single transition that uses both — e.g. CreateFile that introduces
        // a new node and a new dangling transclusion target in one shot —
        // doesn't accidentally collapse them into the same identifier and
        // resolve what was meant to be dangling.
        let max_seen = existing_identifiers
            .iter()
            .chain(missing_identifiers.iter())
            .map(|MockNodeIdentifier(n)| *n)
            .max();
        let next_node_identifier = MockNodeIdentifier(max_seen.map_or(0, |n| n + 1));
        let next_missing_node_identifier = MockNodeIdentifier(max_seen.map_or(1, |n| n + 2));

        let mut existing_sorted: Vec<MockNodeIdentifier> =
            existing_identifiers.into_iter().collect();
        existing_sorted.sort_by_key(|MockNodeIdentifier(n)| *n);

        let mut missing_sorted: Vec<MockNodeIdentifier> = missing_identifiers.into_iter().collect();
        missing_sorted.sort_by_key(|MockNodeIdentifier(n)| *n);

        Queries {
            next_file_id,
            existing_file_ids: Cow::Owned(existing_file_ids),
            existing_file_node_pairs: Cow::Owned(existing_file_node_pairs),
            existing_file_node_transclusion_triples: Cow::Owned(
                existing_file_node_transclusion_triples,
            ),
            existing_file_node_link_triples: Cow::Owned(existing_file_node_link_triples),
            next_node_id,
            next_node_identifier,
            existing_node_identifiers: Cow::Owned(existing_sorted),
            missing_node_identifiers: Cow::Owned(missing_sorted),
            next_missing_node_identifier,
        }
    }

    // fn create_file_strategy(&self, queries: &Queries) -> impl Strategy<Value = CreateFile> + use<> {
    //     let file_id = queries.next_file_id;

    //     // TODO:
    //     mock_file_strategy().prop_map(move |file| CreateFile { file_id, file })
    // }

    // fn replace_file_strategy(
    //     &self,
    //     queries: &Queries,
    // ) -> Option<impl Strategy<Value = ReplaceFile> + use<>> {
    //     if !queries.existing_file_ids.is_empty() {
    //         Some(
    //             (
    //                 select(queries.existing_file_ids.clone()),
    //                 mock_file_strategy(),
    //             )
    //                 .prop_map(move |(file_id, file)| ReplaceFile { file_id, file }),
    //         )
    //     } else {
    //         None
    //     }
    // }

    // // TODO: Is it worth trying to generate missing removes?
    // fn remove_file_strategy(
    //     &self,
    //     queries: &Queries,
    // ) -> Option<impl Strategy<Value = RemoveFile> + use<>> {
    //     if !queries.existing_file_ids.is_empty() {
    //         Some(
    //             select(queries.existing_file_ids.clone())
    //                 .prop_map(|file_id| RemoveFile { file_id }),
    //         )
    //     } else {
    //         None
    //     }
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

impl Transition {
    pub fn file_id(&self) -> FileId {
        match self {
            Transition::CreateFile(create_file) => create_file.file_id,
            Transition::ReplaceFile(replace_file) => replace_file.file_id,
            Transition::RemoveFile(remove_file) => remove_file.file_id,
            Transition::AddNode(add_node) => add_node.file_id,
            Transition::RemoveNode(remove_node) => remove_node.file_id,
            Transition::AddTransclusion(add_transclusion) => add_transclusion.file_id,
            Transition::RemoveTransclusion(remove_transclusion) => remove_transclusion.file_id,
            Transition::AddLink(add_link) => add_link.file_id,
            Transition::RemoveLink(remove_link) => remove_link.file_id,
            Transition::UpdateTitle(update_title) => update_title.file_id,
            Transition::UpdateBody(update_body) => update_body.file_id,
            Transition::EditMetadata(edit_metadata) => edit_metadata.file_id,
            Transition::UpdateLinkTarget(update_link_target) => update_link_target.file_id,
            Transition::UpdateLinkContent(update_link_content) => update_link_content.file_id,
            Transition::UpdateTransclusionTarget(update_transclusion_target) => {
                update_transclusion_target.file_id
            }
            Transition::AddCompileError(add_compile_error) => add_compile_error.file_id,
            Transition::RemoveCompileError(remove_compile_error) => remove_compile_error.file_id,
            Transition::AddCompileWarning(add_compile_warning) => add_compile_warning.file_id,
            Transition::RemoveCompileWarning(remove_compile_warning) => {
                remove_compile_warning.file_id
            }
            Transition::RenameNode(rename_node) => rename_node.file_id,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CreateFile {
    pub file_id: FileId,
    pub file: MockFile,
}

#[derive(Debug, Clone)]
pub struct ReplaceFile {
    pub file_id: FileId,
    pub file: MockFile,
}

#[derive(Debug, Clone)]
pub struct RemoveFile {
    pub file_id: FileId,
}

#[derive(Debug, Clone)]
pub struct AddNode {
    pub file_id: FileId,
    pub node_id: MockNodeId,
    pub node: MockNode,
}

#[derive(Debug, Clone)]
pub struct RemoveNode {
    pub file_id: FileId,
    pub node_id: MockNodeId,
}

#[derive(Debug, Clone)]
pub struct AddTransclusion {
    pub file_id: FileId,
    pub node_id: MockNodeId,
    pub transclusion: MockTransclusion,
}

#[derive(Debug, Clone)]
pub struct RemoveTransclusion {
    pub file_id: FileId,
    pub node_id: MockNodeId,
    pub index: u32,
}

#[derive(Debug, Clone)]
pub struct AddLink {
    pub file_id: FileId,
    pub node_id: MockNodeId,
    pub link: MockLink,
}

#[derive(Debug, Clone)]
pub struct RemoveLink {
    pub file_id: FileId,
    pub node_id: MockNodeId,
    pub index: u32,
}

#[derive(Debug, Clone)]
pub struct UpdateTitle {
    pub file_id: FileId,
    pub node_id: MockNodeId,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct UpdateBody {
    pub file_id: FileId,
    pub node_id: MockNodeId,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct EditMetadata {
    pub file_id: FileId,
    pub node_id: MockNodeId,
    pub target: MetadataTarget,
    pub operation: MetadataOperation,
}

#[derive(Debug, Clone)]
pub struct UpdateLinkTarget {
    pub file_id: FileId,
    pub node_id: MockNodeId,
    pub link_index: u32,
    pub new_target: MockNodeIdentifier,
}

#[derive(Debug, Clone)]
pub struct UpdateLinkContent {
    pub file_id: FileId,
    pub node_id: MockNodeId,
    pub link_index: u32,
    pub new_content: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UpdateTransclusionTarget {
    pub file_id: FileId,
    pub node_id: MockNodeId,
    pub transclusion_index: u32,
    pub new_target: MockNodeIdentifier,
}

#[derive(Debug, Clone)]
pub struct AddCompileError {
    pub file_id: FileId,
    pub error: String,
}

#[derive(Debug, Clone)]
pub struct RemoveCompileError {
    pub file_id: FileId,
    pub index: usize,
}

#[derive(Debug, Clone)]
pub struct AddCompileWarning {
    pub file_id: FileId,
    pub warning: String,
}

#[derive(Debug, Clone)]
pub struct RemoveCompileWarning {
    pub file_id: FileId,
    pub index: usize,
}

#[derive(Debug, Clone)]
pub struct RenameNode {
    pub file_id: FileId,
    pub node_id: MockNodeId,
    pub new_id: MockNodeIdentifier,
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
    next_file_id: FileId,
    existing_file_ids: Cow<'static, [FileId]>,
    existing_file_node_pairs: Cow<'static, [(FileId, MockNodeId)]>,
    existing_file_node_transclusion_triples: Cow<'static, [(FileId, MockNodeId, u32)]>,
    existing_file_node_link_triples: Cow<'static, [(FileId, MockNodeId, u32)]>,
    next_node_id: MockNodeId,
    next_node_identifier: MockNodeIdentifier,
    existing_node_identifiers: Cow<'static, [MockNodeIdentifier]>,
    missing_node_identifiers: Cow<'static, [MockNodeIdentifier]>,
    next_missing_node_identifier: MockNodeIdentifier,
}

const CREATE_FILE_NODE_MAX: usize = 5;
const CREATE_FILE_COMPILE_ERRORS_MAX: usize = 3;
const CREATE_FILE_COMPILE_WARNINGS_MAX: usize = 3;
const METADATA_ENTRIES_MAX: usize = 6;
const METADATA_VALUES_MAX: usize = 4;
const NODE_TRANSCLUSIONS_MAX: usize = 5;
const NODE_LINKS_MAX: usize = 5;

fn mock_file_strategy(queries: &Queries) -> impl Strategy<Value = MockFile> + use<> {
    let next_node_id_value = queries.next_node_id.0;

    (
        vec(mock_node_strategy(queries), 0..CREATE_FILE_NODE_MAX),
        vec(compile_error_strategy(), 0..CREATE_FILE_COMPILE_ERRORS_MAX),
        vec(
            compile_warning_strategy(),
            0..CREATE_FILE_COMPILE_WARNINGS_MAX,
        ),
    )
        .prop_map(move |(nodes, errors, warnings)| MockFile {
            nodes: (next_node_id_value..).map(MockNodeId).zip(nodes).collect(),
            errors,
            warnings,
        })
}

fn mock_node_strategy(queries: &Queries) -> impl Strategy<Value = MockNode> + use<> {
    (
        node_identifier_strategy(queries),
        title_strategy(),
        body_strategy(),
        span_strategy(),
        metadata_strategy(),
        vec(
            mock_transclusion_strategy(queries),
            0..NODE_TRANSCLUSIONS_MAX,
        ),
        vec(mock_link_strategy(queries), 0..NODE_LINKS_MAX),
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

fn mock_link_strategy(queries: &Queries) -> impl Strategy<Value = MockLink> + use<> {
    (
        target_strategy(queries),
        option::of(link_content_strategy()),
        metadata_strategy(),
    )
        .prop_map(|(target, content, metadata)| MockLink {
            target,
            content,
            metadata,
        })
}

fn mock_transclusion_strategy(
    queries: &Queries,
) -> impl Strategy<Value = MockTransclusion> + use<> {
    (target_strategy(queries), metadata_strategy())
        .prop_map(|(target, metadata)| MockTransclusion { target, metadata })
}

fn target_strategy(queries: &Queries) -> impl Strategy<Value = MockNodeIdentifier> + use<> {
    let next_missing = Just(queries.next_missing_node_identifier).boxed();
    let mut strategies = vec![next_missing];

    if !queries.existing_node_identifiers.is_empty() {
        strategies.push(select(queries.existing_node_identifiers.clone()).boxed());
    }

    if !queries.missing_node_identifiers.is_empty() {
        strategies.push(select(queries.missing_node_identifiers.clone()).boxed());
    }

    Union::new(strategies)
}

fn node_identifier_strategy(
    queries: &Queries,
) -> impl Strategy<Value = MockNodeIdentifier> + use<> {
    let next = Just(queries.next_node_identifier).boxed();
    let mut strategies = vec![next];

    if !queries.existing_node_identifiers.is_empty() {
        strategies.push(select(queries.existing_node_identifiers.clone()).boxed());
    }

    if !queries.missing_node_identifiers.is_empty() {
        strategies.push(select(queries.missing_node_identifiers.clone()).boxed());
    }

    Union::new(strategies)
}

fn file_id_strategy() -> impl Strategy<Value = FileId> {
    any::<u16>()
        .prop_filter_map("FileIds need to be nonzero", NonZeroU16::new)
        .prop_map(FileId::from_raw)
}

fn span_strategy() -> impl Strategy<Value = Span> {
    (file_id_strategy(), range_strategy())
        .prop_map(|(file_id, range)| Span::from_range(file_id, range))
}

fn metadata_strategy() -> impl Strategy<Value = Metadata> {
    hash_map(
        metadata_key_strategy(),
        vec(metadata_key_strategy(), 0..=METADATA_VALUES_MAX),
        0..=METADATA_ENTRIES_MAX,
    )
}

// Helpers

fn body_strategy() -> impl Strategy<Value = String> {
    "[a-z]*"
}

fn title_strategy() -> impl Strategy<Value = String> {
    "[a-z]*"
}

fn metadata_key_strategy() -> impl Strategy<Value = String> {
    "[a-z]*"
}

fn link_content_strategy() -> impl Strategy<Value = String> {
    "[a-z]+"
}

fn compile_error_strategy() -> impl Strategy<Value = String> {
    "[a-z]+"
}

fn compile_warning_strategy() -> impl Strategy<Value = String> {
    "[a-z]+"
}

fn range_strategy() -> impl Strategy<Value = Range<usize>> {
    any::<(usize, usize)>().prop_map(|(a, b)| {
        let start = a.min(b);
        let end = a.max(b);
        start..end
    })
}

trait Event {
    fn strategy(
        state: &State,
        queries: &Queries,
    ) -> Option<impl Strategy<Value = Self> + use<Self>>;

    fn does_apply(&self, state: &State) -> bool;

    fn apply(&self, state: State) -> State;
}

impl Event for CreateFile {
    fn strategy(_state: &State, queries: &Queries) -> Option<impl Strategy<Value = Self> + use<>> {
        let file_id = queries.next_file_id;

        Some(mock_file_strategy(queries).prop_map(move |file| CreateFile { file_id, file }))
    }

    fn does_apply(&self, _state: &State) -> bool {
        true
    }

    fn apply(&self, mut state: State) -> State {
        let CreateFile {
            file_id,
            file:
                MockFile {
                    nodes,
                    errors,
                    warnings,
                },
        } = self.clone();

        state.files.insert(
            file_id,
            FileState {
                nodes: nodes.keys().copied().collect(),
                errors,
                warnings,
            },
        );
        state.nodes.extend(nodes);

        state
    }
}

impl Event for ReplaceFile {
    fn strategy(_state: &State, queries: &Queries) -> Option<impl Strategy<Value = Self> + use<>> {
        if queries.existing_file_ids.is_empty() {
            None
        } else {
            Some(
                (
                    select(queries.existing_file_ids.clone()),
                    mock_file_strategy(queries),
                )
                    .prop_map(|(file_id, file)| ReplaceFile { file_id, file }),
            )
        }
    }

    fn does_apply(&self, state: &State) -> bool {
        state.files.contains_key(&self.file_id)
    }

    fn apply(&self, mut state: State) -> State {
        let ReplaceFile {
            file_id,
            file:
                MockFile {
                    nodes,
                    errors,
                    warnings,
                },
        } = self.clone();

        state.remove_file_nodes(file_id);
        state.files.insert(
            file_id,
            FileState {
                nodes: nodes.keys().copied().collect(),
                errors,
                warnings,
            },
        );
        state.nodes.extend(nodes);

        state
    }
}

impl Event for RemoveFile {
    fn strategy(_state: &State, queries: &Queries) -> Option<impl Strategy<Value = Self> + use<>> {
        if queries.existing_file_ids.is_empty() {
            None
        } else {
            Some(
                select(queries.existing_file_ids.clone())
                    .prop_map(|file_id| RemoveFile { file_id }),
            )
        }
    }

    fn does_apply(&self, state: &State) -> bool {
        state.files.contains_key(&self.file_id)
    }

    fn apply(&self, mut state: State) -> State {
        let RemoveFile { file_id } = self;

        state.remove_file_nodes(*file_id);
        let removed = state.files.remove(file_id);
        assert!(
            removed.is_some(),
            "bug: file disappeared before RemoveFile::apply removed it"
        );

        state
    }
}

impl Event for AddNode {
    fn strategy(_state: &State, queries: &Queries) -> Option<impl Strategy<Value = Self> + use<>> {
        if queries.existing_file_ids.is_empty() {
            None
        } else {
            let node_id = queries.next_node_id;
            Some(
                (
                    select(queries.existing_file_ids.clone()),
                    mock_node_strategy(queries),
                )
                    .prop_map(move |(file_id, node)| AddNode {
                        file_id,
                        node_id,
                        node,
                    }),
            )
        }
    }

    fn does_apply(&self, state: &State) -> bool {
        state.files.contains_key(&self.file_id) && !state.nodes.contains_key(&self.node_id)
    }

    fn apply(&self, mut state: State) -> State {
        let AddNode {
            file_id,
            node_id,
            node,
        } = self.clone();

        let replaced = state.nodes.insert(node_id, node);
        assert!(
            replaced.is_none(),
            "bug: AddNode inserted duplicate MockNodeId"
        );

        let file = state
            .files
            .get_mut(&file_id)
            .expect("bug: AddNode target file disappeared before apply");
        let inserted = file.nodes.insert(node_id);
        assert!(
            inserted,
            "bug: AddNode inserted a node id already owned by the target file"
        );

        state
    }
}

impl Event for RemoveNode {
    fn strategy(_state: &State, queries: &Queries) -> Option<impl Strategy<Value = Self> + use<>> {
        if queries.existing_file_node_pairs.is_empty() {
            None
        } else {
            Some(
                select(queries.existing_file_node_pairs.clone())
                    .prop_map(|(file_id, node_id)| RemoveNode { file_id, node_id }),
            )
        }
    }

    fn does_apply(&self, state: &State) -> bool {
        state.files.contains_key(&self.file_id) && state.nodes.contains_key(&self.node_id)
    }

    fn apply(&self, mut state: State) -> State {
        let RemoveNode { file_id, node_id } = self;

        let file = state
            .files
            .get_mut(file_id)
            .expect("bug: RemoveNode target file disappeared before apply");
        let removed_from_file = file.nodes.remove(node_id);
        assert!(
            removed_from_file,
            "bug: RemoveNode targeted a node id not owned by the target file"
        );

        let removed_from_nodes = state.nodes.remove(node_id);
        assert!(
            removed_from_nodes.is_some(),
            "bug: RemoveNode targeted a node id missing from the global node store"
        );

        state
    }
}

impl Event for AddTransclusion {
    fn strategy(_state: &State, queries: &Queries) -> Option<impl Strategy<Value = Self> + use<>> {
        if queries.existing_file_node_pairs.is_empty() {
            None
        } else {
            Some(
                (
                    select(queries.existing_file_node_pairs.clone()),
                    mock_transclusion_strategy(queries),
                )
                    .prop_map(|((file_id, node_id), transclusion)| {
                        AddTransclusion {
                            file_id,
                            node_id,
                            transclusion,
                        }
                    }),
            )
        }
    }

    fn does_apply(&self, state: &State) -> bool {
        state.files.contains_key(&self.file_id) && state.nodes.contains_key(&self.node_id)
    }

    fn apply(&self, mut state: State) -> State {
        let AddTransclusion {
            file_id: _,
            node_id,
            transclusion,
        } = self;

        let node = state
            .nodes
            .get_mut(node_id)
            .expect("bug: AddTransclusion target node disappeared before apply");
        node.transclusions.push(transclusion.clone());

        state
    }
}

impl Event for RemoveTransclusion {
    fn strategy(_state: &State, queries: &Queries) -> Option<impl Strategy<Value = Self> + use<>> {
        if queries.existing_file_node_transclusion_triples.is_empty() {
            None
        } else {
            Some(
                select(queries.existing_file_node_transclusion_triples.clone()).prop_map(
                    |(file_id, node_id, index)| RemoveTransclusion {
                        file_id,
                        node_id,
                        index,
                    },
                ),
            )
        }
    }

    fn does_apply(&self, state: &State) -> bool {
        state.files.contains_key(&self.file_id)
            && state
                .nodes
                .get(&self.node_id)
                .is_some_and(|node| self.index < node.transclusions.len() as u32)
    }

    fn apply(&self, mut state: State) -> State {
        let RemoveTransclusion {
            file_id: _,
            node_id,
            index,
        } = self;

        let node = state
            .nodes
            .get_mut(node_id)
            .expect("bug: RemoveTransclusion target node disappeared before apply");
        node.transclusions.remove(*index as usize);

        state
    }
}

impl Event for AddLink {
    fn strategy(_state: &State, queries: &Queries) -> Option<impl Strategy<Value = Self> + use<>> {
        if queries.existing_file_node_pairs.is_empty() {
            None
        } else {
            Some(
                (
                    select(queries.existing_file_node_pairs.clone()),
                    mock_link_strategy(queries),
                )
                    .prop_map(|((file_id, node_id), link)| AddLink {
                        file_id,
                        node_id,
                        link,
                    }),
            )
        }
    }

    fn does_apply(&self, state: &State) -> bool {
        state.files.contains_key(&self.file_id) && state.nodes.contains_key(&self.node_id)
    }

    fn apply(&self, mut state: State) -> State {
        let AddLink {
            file_id: _,
            node_id,
            link,
        } = self;

        let node = state
            .nodes
            .get_mut(node_id)
            .expect("bug: AddLink target node disappeared before apply");
        node.links.push(link.clone());

        state
    }
}

impl Event for RemoveLink {
    fn strategy(_state: &State, queries: &Queries) -> Option<impl Strategy<Value = Self> + use<>> {
        if queries.existing_file_node_link_triples.is_empty() {
            None
        } else {
            Some(
                select(queries.existing_file_node_link_triples.clone()).prop_map(
                    |(file_id, node_id, index)| RemoveLink {
                        file_id,
                        node_id,
                        index,
                    },
                ),
            )
        }
    }

    fn does_apply(&self, state: &State) -> bool {
        state.files.contains_key(&self.file_id)
            && state
                .nodes
                .get(&self.node_id)
                .is_some_and(|node| self.index < node.links.len() as u32)
    }

    fn apply(&self, mut state: State) -> State {
        let RemoveLink {
            file_id: _,
            node_id,
            index,
        } = self;

        let node = state
            .nodes
            .get_mut(node_id)
            .expect("bug: RemoveLink target node disappeared before apply");
        node.links.remove(*index as usize);

        state
    }
}

impl Event for UpdateTitle {
    fn strategy(_state: &State, queries: &Queries) -> Option<impl Strategy<Value = Self> + use<>> {
        if queries.existing_file_node_pairs.is_empty() {
            None
        } else {
            Some(
                (
                    select(queries.existing_file_node_pairs.clone()),
                    title_strategy(),
                )
                    .prop_map(|((file_id, node_id), title)| UpdateTitle {
                        file_id,
                        node_id,
                        title,
                    }),
            )
        }
    }

    fn does_apply(&self, state: &State) -> bool {
        state.files.contains_key(&self.file_id) && state.nodes.contains_key(&self.node_id)
    }

    fn apply(&self, mut state: State) -> State {
        let UpdateTitle {
            file_id: _,
            node_id,
            title,
        } = self;

        let node = state
            .nodes
            .get_mut(node_id)
            .expect("bug: UpdateTitle target node disappeared before apply");
        node.title = title.clone();

        state
    }
}

impl Event for UpdateBody {
    fn strategy(_state: &State, queries: &Queries) -> Option<impl Strategy<Value = Self> + use<>> {
        if queries.existing_file_node_pairs.is_empty() {
            None
        } else {
            Some(
                (
                    select(queries.existing_file_node_pairs.clone()),
                    body_strategy(),
                )
                    .prop_map(|((file_id, node_id), body)| UpdateBody {
                        file_id,
                        node_id,
                        body,
                    }),
            )
        }
    }

    fn does_apply(&self, state: &State) -> bool {
        state.files.contains_key(&self.file_id) && state.nodes.contains_key(&self.node_id)
    }

    fn apply(&self, mut state: State) -> State {
        let UpdateBody {
            file_id: _,
            node_id,
            body,
        } = self;

        let node = state
            .nodes
            .get_mut(node_id)
            .expect("bug: UpdateBody target node disappeared before apply");
        node.body = body.clone();

        state
    }
}

impl Event for UpdateLinkTarget {
    fn strategy(_state: &State, queries: &Queries) -> Option<impl Strategy<Value = Self> + use<>> {
        if queries.existing_file_node_link_triples.is_empty() {
            None
        } else {
            Some(
                (
                    select(queries.existing_file_node_link_triples.clone()),
                    target_strategy(queries),
                )
                    .prop_map(|((file_id, node_id, link_index), new_target)| {
                        UpdateLinkTarget {
                            file_id,
                            node_id,
                            link_index,
                            new_target,
                        }
                    }),
            )
        }
    }

    fn does_apply(&self, state: &State) -> bool {
        state.files.contains_key(&self.file_id)
            && state
                .nodes
                .get(&self.node_id)
                .is_some_and(|node| self.link_index < node.links.len() as u32)
    }

    fn apply(&self, mut state: State) -> State {
        let UpdateLinkTarget {
            file_id: _,
            node_id,
            link_index,
            new_target,
        } = self;

        let node = state
            .nodes
            .get_mut(node_id)
            .expect("bug: UpdateLinkTarget target node disappeared before apply");
        let link = node
            .links
            .get_mut(*link_index as usize)
            .expect("bug: UpdateLinkTarget target link disappeared before apply");
        link.target = *new_target;

        state
    }
}

impl Event for UpdateLinkContent {
    fn strategy(_state: &State, queries: &Queries) -> Option<impl Strategy<Value = Self> + use<>> {
        if queries.existing_file_node_link_triples.is_empty() {
            None
        } else {
            Some(
                (
                    select(queries.existing_file_node_link_triples.clone()),
                    option::of(link_content_strategy()),
                )
                    .prop_map(|((file_id, node_id, link_index), new_content)| {
                        UpdateLinkContent {
                            file_id,
                            node_id,
                            link_index,
                            new_content,
                        }
                    }),
            )
        }
    }

    fn does_apply(&self, state: &State) -> bool {
        state.files.contains_key(&self.file_id)
            && state
                .nodes
                .get(&self.node_id)
                .is_some_and(|node| self.link_index < node.links.len() as u32)
    }

    fn apply(&self, mut state: State) -> State {
        let UpdateLinkContent {
            file_id: _,
            node_id,
            link_index,
            new_content,
        } = self;

        let node = state
            .nodes
            .get_mut(node_id)
            .expect("bug: UpdateLinkContent target node disappeared before apply");
        let link = node
            .links
            .get_mut(*link_index as usize)
            .expect("bug: UpdateLinkContent target link disappeared before apply");
        link.content = new_content.clone();

        state
    }
}

impl Event for UpdateTransclusionTarget {
    fn strategy(_state: &State, queries: &Queries) -> Option<impl Strategy<Value = Self> + use<>> {
        if queries.existing_file_node_transclusion_triples.is_empty() {
            None
        } else {
            Some(
                (
                    select(queries.existing_file_node_transclusion_triples.clone()),
                    target_strategy(queries),
                )
                    .prop_map(
                        |((file_id, node_id, transclusion_index), new_target)| {
                            UpdateTransclusionTarget {
                                file_id,
                                node_id,
                                transclusion_index,
                                new_target,
                            }
                        },
                    ),
            )
        }
    }

    fn does_apply(&self, state: &State) -> bool {
        state.files.contains_key(&self.file_id)
            && state
                .nodes
                .get(&self.node_id)
                .is_some_and(|node| self.transclusion_index < node.transclusions.len() as u32)
    }

    fn apply(&self, mut state: State) -> State {
        let UpdateTransclusionTarget {
            file_id: _,
            node_id,
            transclusion_index,
            new_target,
        } = self;

        let node = state
            .nodes
            .get_mut(node_id)
            .expect("bug: UpdateTransclusionTarget target node disappeared before apply");
        let transclusion = node
            .transclusions
            .get_mut(*transclusion_index as usize)
            .expect("bug: UpdateTransclusionTarget target transclusion disappeared before apply");
        transclusion.target = *new_target;

        state
    }
}

fn state_strategy() -> impl Strategy<Value = State> {
    ReferenceCompiler::sequential_strategy(0..20usize).prop_map(|(initial, transitions, _)| {
        transitions.iter().fold(initial, ReferenceCompiler::apply)
    })
}

fn state_and_next_transition_strategy() -> impl Strategy<Value = (State, Transition)> {
    state_strategy().prop_flat_map(|state| {
        let transition = ReferenceCompiler::transitions(&state);

        (Just(state), transition)
    })
}

proptest! {
    #[test]
    fn transitions_satisfy_preconditions(
        (state, transition) in state_and_next_transition_strategy()
    ) {
        prop_assert!(
            ReferenceCompiler::preconditions(&state, &transition),
            "{transition:?} fails preconditions in {state:?}",
        );
    }

    #[test]
    fn create_file_strategy_implies_does_apply(
        (state, create_file) in state_strategy().prop_flat_map(|state| {
            let queries = state.queries();
            let strategy = CreateFile::strategy(&state, &queries)
                .expect("CreateFile::strategy always returns Some");
            (Just(state), strategy)
        })
    ) {
        prop_assert!(
            create_file.does_apply(&state),
            "{create_file:?} fails does_apply in {state:?}",
        );
    }

    #[test]
    fn replace_file_strategy_implies_does_apply(
        (state, replace_file) in state_strategy()
            .prop_filter("ReplaceFile needs an existing file", |state| !state.files.is_empty())
            .prop_flat_map(|state| {
                let queries = state.queries();
                let strategy = ReplaceFile::strategy(&state, &queries)
                    .expect("ReplaceFile::strategy returns Some when files exist");
                (Just(state), strategy)
            })
    ) {
        prop_assert!(
            replace_file.does_apply(&state),
            "{replace_file:?} fails does_apply in {state:?}",
        );
    }

    #[test]
    fn remove_file_strategy_implies_does_apply(
        (state, remove_file) in state_strategy()
            .prop_filter("RemoveFile needs an existing file", |state| !state.files.is_empty())
            .prop_flat_map(|state| {
                let queries = state.queries();
                let strategy = RemoveFile::strategy(&state, &queries)
                    .expect("RemoveFile::strategy returns Some when files exist");
                (Just(state), strategy)
            })
    ) {
        prop_assert!(
            remove_file.does_apply(&state),
            "{remove_file:?} fails does_apply in {state:?}",
        );
    }

    #[test]
    fn add_node_strategy_implies_does_apply(
        (state, add_node) in state_strategy()
            .prop_filter("AddNode needs an existing file", |state| !state.files.is_empty())
            .prop_flat_map(|state| {
                let queries = state.queries();
                let strategy = AddNode::strategy(&state, &queries)
                    .expect("AddNode::strategy returns Some when files exist");
                (Just(state), strategy)
            })
    ) {
        prop_assert!(
            add_node.does_apply(&state),
            "{add_node:?} fails does_apply in {state:?}",
        );
    }

    #[test]
    fn remove_node_strategy_implies_does_apply(
        (state, remove_node) in state_strategy()
            .prop_filter("RemoveNode needs an existing node", |state| !state.nodes.is_empty())
            .prop_flat_map(|state| {
                let queries = state.queries();
                let strategy = RemoveNode::strategy(&state, &queries)
                    .expect("RemoveNode::strategy returns Some when nodes exist");
                (Just(state), strategy)
            })
    ) {
        prop_assert!(
            remove_node.does_apply(&state),
            "{remove_node:?} fails does_apply in {state:?}",
        );
    }

    #[test]
    fn add_transclusion_strategy_implies_does_apply(
        (state, add_transclusion) in state_strategy()
            .prop_filter("AddTransclusion needs an existing node", |state| !state.nodes.is_empty())
            .prop_flat_map(|state| {
                let queries = state.queries();
                let strategy = AddTransclusion::strategy(&state, &queries)
                    .expect("AddTransclusion::strategy returns Some when nodes exist");
                (Just(state), strategy)
            })
    ) {
        prop_assert!(
            add_transclusion.does_apply(&state),
            "{add_transclusion:?} fails does_apply in {state:?}",
        );
    }

    #[test]
    fn remove_transclusion_strategy_implies_does_apply(
        (state, remove_transclusion) in state_strategy()
            .prop_filter(
                "RemoveTransclusion needs an existing transclusion",
                |state| state.nodes.values().any(|node| !node.transclusions.is_empty()),
            )
            .prop_flat_map(|state| {
                let queries = state.queries();
                let strategy = RemoveTransclusion::strategy(&state, &queries)
                    .expect("RemoveTransclusion::strategy returns Some when transclusions exist");
                (Just(state), strategy)
            })
    ) {
        prop_assert!(
            remove_transclusion.does_apply(&state),
            "{remove_transclusion:?} fails does_apply in {state:?}",
        );
    }

    #[test]
    fn add_link_strategy_implies_does_apply(
        (state, add_link) in state_strategy()
            .prop_filter("AddLink needs an existing node", |state| !state.nodes.is_empty())
            .prop_flat_map(|state| {
                let queries = state.queries();
                let strategy = AddLink::strategy(&state, &queries)
                    .expect("AddLink::strategy returns Some when nodes exist");
                (Just(state), strategy)
            })
    ) {
        prop_assert!(
            add_link.does_apply(&state),
            "{add_link:?} fails does_apply in {state:?}",
        );
    }

    #[test]
    fn remove_link_strategy_implies_does_apply(
        (state, remove_link) in state_strategy()
            .prop_filter(
                "RemoveLink needs an existing link",
                |state| state.nodes.values().any(|node| !node.links.is_empty()),
            )
            .prop_flat_map(|state| {
                let queries = state.queries();
                let strategy = RemoveLink::strategy(&state, &queries)
                    .expect("RemoveLink::strategy returns Some when links exist");
                (Just(state), strategy)
            })
    ) {
        prop_assert!(
            remove_link.does_apply(&state),
            "{remove_link:?} fails does_apply in {state:?}",
        );
    }

    #[test]
    fn update_title_strategy_implies_does_apply(
        (state, update_title) in state_strategy()
            .prop_filter("UpdateTitle needs an existing node", |state| !state.nodes.is_empty())
            .prop_flat_map(|state| {
                let queries = state.queries();
                let strategy = UpdateTitle::strategy(&state, &queries)
                    .expect("UpdateTitle::strategy returns Some when nodes exist");
                (Just(state), strategy)
            })
    ) {
        prop_assert!(
            update_title.does_apply(&state),
            "{update_title:?} fails does_apply in {state:?}",
        );
    }

    #[test]
    fn update_body_strategy_implies_does_apply(
        (state, update_body) in state_strategy()
            .prop_filter("UpdateBody needs an existing node", |state| !state.nodes.is_empty())
            .prop_flat_map(|state| {
                let queries = state.queries();
                let strategy = UpdateBody::strategy(&state, &queries)
                    .expect("UpdateBody::strategy returns Some when nodes exist");
                (Just(state), strategy)
            })
    ) {
        prop_assert!(
            update_body.does_apply(&state),
            "{update_body:?} fails does_apply in {state:?}",
        );
    }

    #[test]
    fn update_link_target_strategy_implies_does_apply(
        (state, update_link_target) in state_strategy()
            .prop_filter(
                "UpdateLinkTarget needs an existing link",
                |state| state.nodes.values().any(|node| !node.links.is_empty()),
            )
            .prop_flat_map(|state| {
                let queries = state.queries();
                let strategy = UpdateLinkTarget::strategy(&state, &queries)
                    .expect("UpdateLinkTarget::strategy returns Some when links exist");
                (Just(state), strategy)
            })
    ) {
        prop_assert!(
            update_link_target.does_apply(&state),
            "{update_link_target:?} fails does_apply in {state:?}",
        );
    }

    #[test]
    fn update_link_content_strategy_implies_does_apply(
        (state, update_link_content) in state_strategy()
            .prop_filter(
                "UpdateLinkContent needs an existing link",
                |state| state.nodes.values().any(|node| !node.links.is_empty()),
            )
            .prop_flat_map(|state| {
                let queries = state.queries();
                let strategy = UpdateLinkContent::strategy(&state, &queries)
                    .expect("UpdateLinkContent::strategy returns Some when links exist");
                (Just(state), strategy)
            })
    ) {
        prop_assert!(
            update_link_content.does_apply(&state),
            "{update_link_content:?} fails does_apply in {state:?}",
        );
    }

    #[test]
    fn update_transclusion_target_strategy_implies_does_apply(
        (state, update_transclusion_target) in state_strategy()
            .prop_filter(
                "UpdateTransclusionTarget needs an existing transclusion",
                |state| state.nodes.values().any(|node| !node.transclusions.is_empty()),
            )
            .prop_flat_map(|state| {
                let queries = state.queries();
                let strategy = UpdateTransclusionTarget::strategy(&state, &queries)
                    .expect("UpdateTransclusionTarget::strategy returns Some when transclusions exist");
                (Just(state), strategy)
            })
    ) {
        prop_assert!(
            update_transclusion_target.does_apply(&state),
            "{update_transclusion_target:?} fails does_apply in {state:?}",
        );
    }
}
