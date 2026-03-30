use std::collections::{HashMap, HashSet};
use std::num::NonZeroU16;
use std::ops::RangeInclusive;

use ecow::{EcoString, EcoVec};
use proptest::collection::{hash_set, vec};
use proptest::prelude::*;
use proptest::sample::select;
use typst::diag::{SourceDiagnostic, Warned};
use typst::syntax::{FileId, Span};

use crate::compiler::{Compile, CompileOutput, Metadata};

#[derive(Debug, Clone)]
pub struct MockNode {
    pub identifier: String,
    pub title: String,
    pub metadata: Metadata,
    pub body: Vec<MockElement>,
}

#[derive(Debug, Clone)]
pub struct MockSubnode {
    pub identifier: String,
    pub title: String,
    pub metadata: Metadata,
    pub body: Vec<MockElement>,
    pub transclude: bool,
}

#[derive(Debug, Clone)]
pub enum MockElement {
    Text(String),
    Link {
        target: String,
        content: String,
        metadata: Metadata,
    },
    Transclusion {
        target: String,
        metadata: Metadata,
    },
}

#[derive(Debug, Clone)]
pub enum MockFileMode {
    WellFormed,
    MissingPrimaryNode,
    DuplicatePrimaryNode,
    MissingPrimaryTitle,
    InvalidSubnodeTransclude,
    CompileError(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct MockFile {
    pub primary: MockNode,
    pub subnodes: Vec<MockSubnode>,
    pub warnings: Vec<String>,
    pub mode: MockFileMode,
}

impl MockFile {
    pub fn apply_mutation(&mut self, mutation: &FileMutation) {
        match mutation {
            FileMutation::ChangePrimaryTitle(title) => {
                self.primary.title = title.clone();
            }
            FileMutation::ChangePrimaryMetadata(metadata) => {
                self.primary.metadata = metadata.clone();
            }
            FileMutation::ChangePrimaryBody(body) => {
                self.primary.body = body.clone();
            }
            FileMutation::RenamePrimary(identifier) => {
                self.primary.identifier = identifier.clone();
            }
            FileMutation::AddSubnode(subnode) => {
                self.subnodes.push(subnode.clone());
            }
            FileMutation::RemoveLastSubnode => {
                self.subnodes.pop();
            }
            FileMutation::UpdateFirstSubnode(mutation) => {
                if let Some(subnode) = self.subnodes.first_mut() {
                    match mutation {
                        SubnodeMutation::Rename(identifier) => {
                            subnode.identifier = identifier.clone();
                        }
                        SubnodeMutation::ChangeTitle(title) => {
                            subnode.title = title.clone();
                        }
                        SubnodeMutation::ChangeMetadata(metadata) => {
                            subnode.metadata = metadata.clone();
                        }
                        SubnodeMutation::ChangeBody(body) => {
                            subnode.body = body.clone();
                        }
                        SubnodeMutation::ToggleTransclude => {
                            subnode.transclude = !subnode.transclude;
                        }
                    }
                }
            }
            FileMutation::SetWarnings(warnings) => {
                self.warnings = warnings.clone();
            }
            FileMutation::SetMode(mode) => {
                self.mode = mode.clone();
            }
        }

        self.normalize_identifiers();
    }

    fn lower_output(&self) -> CompileOutput {
        let mut normalized = self.clone();
        normalized.normalize_identifiers();

        let mut spans = HashMap::new();
        let mut metadata = HashMap::new();
        let mut transclusion_metadata = HashMap::new();
        let mut link_metadata = HashMap::new();
        let mut counter = 0u32;

        let html = match &normalized.mode {
            MockFileMode::WellFormed => normalized.render_primary(
                &normalized.primary,
                &normalized.subnodes,
                true,
                None,
                &mut spans,
                &mut metadata,
                &mut transclusion_metadata,
                &mut link_metadata,
                &mut counter,
            ),
            MockFileMode::MissingPrimaryNode => {
                let mut html = String::new();
                for subnode in &normalized.subnodes {
                    let subnode = MockSubnode {
                        transclude: false,
                        ..subnode.clone()
                    };
                    html.push_str(&normalized.render_subnode(
                        &subnode,
                        None,
                        &mut spans,
                        &mut metadata,
                        &mut transclusion_metadata,
                        &mut link_metadata,
                        &mut counter,
                    ));
                }
                html
            }
            MockFileMode::DuplicatePrimaryNode => {
                let mut html = normalized.render_primary(
                    &normalized.primary,
                    &normalized.subnodes,
                    true,
                    None,
                    &mut spans,
                    &mut metadata,
                    &mut transclusion_metadata,
                    &mut link_metadata,
                    &mut counter,
                );
                spans.insert(
                    format!("{}-duplicate", normalized.primary.identifier),
                    Span::detached(),
                );
                html.push_str(&format!(
                    r#"<wb-node identifier="{}-duplicate"><wb-title>{}</wb-title></wb-node>"#,
                    normalized.primary.identifier, normalized.primary.title
                ));
                html
            }
            MockFileMode::MissingPrimaryTitle => {
                spans.insert(normalized.primary.identifier.clone(), Span::detached());
                let mut html = format!(
                    r#"<wb-node identifier="{}">"#,
                    normalized.primary.identifier
                );
                for subnode in &normalized.subnodes {
                    let subnode = MockSubnode {
                        transclude: false,
                        ..subnode.clone()
                    };
                    html.push_str(&normalized.render_subnode(
                        &subnode,
                        None,
                        &mut spans,
                        &mut metadata,
                        &mut transclusion_metadata,
                        &mut link_metadata,
                        &mut counter,
                    ));
                }
                html.push_str("</wb-node>");
                html
            }
            MockFileMode::InvalidSubnodeTransclude => {
                let mut subnodes = normalized.subnodes.clone();
                if subnodes.is_empty() {
                    subnodes.push(MockSubnode {
                        identifier: format!("{}-sub", normalized.primary.identifier),
                        title: normalized.primary.title.clone(),
                        metadata: normalized.primary.metadata.clone(),
                        body: normalized.primary.body.clone(),
                        transclude: true,
                    });
                }
                normalized.render_primary(
                    &normalized.primary,
                    &subnodes,
                    true,
                    Some("maybe"),
                    &mut spans,
                    &mut metadata,
                    &mut transclusion_metadata,
                    &mut link_metadata,
                    &mut counter,
                )
            }
            MockFileMode::CompileError(_) => unreachable!("compile errors do not lower html"),
        };

        CompileOutput {
            html,
            spans,
            metadata,
            transclusion_metadata,
            link_metadata,
            errors: EcoVec::new(),
        }
    }

    fn normalize_identifiers(&mut self) {
        self.normalize_identifiers_against(&HashSet::new());
    }

    pub fn normalize_identifiers_against(&mut self, reserved: &HashSet<String>) {
        let mut used = reserved.clone();

        self.primary.identifier = unique_identifier(&self.primary.identifier, &mut used, "node");
        for (index, subnode) in self.subnodes.iter_mut().enumerate() {
            subnode.identifier =
                unique_identifier(&subnode.identifier, &mut used, &format!("subnode-{index}"));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_primary(
        &self,
        node: &MockNode,
        subnodes: &[MockSubnode],
        include_title: bool,
        invalid_subnode_transclude: Option<&str>,
        spans: &mut HashMap<String, Span>,
        metadata: &mut HashMap<String, Metadata>,
        transclusion_metadata: &mut HashMap<u32, Metadata>,
        link_metadata: &mut HashMap<u32, Metadata>,
        counter: &mut u32,
    ) -> String {
        spans.insert(node.identifier.clone(), Span::detached());
        metadata.insert(node.identifier.clone(), node.metadata.clone());

        let mut html = format!(r#"<wb-node identifier="{}">"#, node.identifier);
        if include_title {
            html.push_str(&format!("<wb-title>{}</wb-title>", node.title));
        }
        for (index, subnode) in subnodes.iter().enumerate() {
            html.push_str(&self.render_subnode(
                subnode,
                if index == 0 {
                    invalid_subnode_transclude
                } else {
                    None
                },
                spans,
                metadata,
                transclusion_metadata,
                link_metadata,
                counter,
            ));
        }
        html.push_str(&render_body(
            &node.body,
            transclusion_metadata,
            link_metadata,
            counter,
        ));
        html.push_str("</wb-node>");
        html
    }

    #[allow(clippy::too_many_arguments)]
    fn render_subnode(
        &self,
        subnode: &MockSubnode,
        transclude_override: Option<&str>,
        spans: &mut HashMap<String, Span>,
        metadata: &mut HashMap<String, Metadata>,
        transclusion_metadata: &mut HashMap<u32, Metadata>,
        link_metadata: &mut HashMap<u32, Metadata>,
        counter: &mut u32,
    ) -> String {
        spans.insert(subnode.identifier.clone(), Span::detached());
        metadata.insert(subnode.identifier.clone(), subnode.metadata.clone());

        let transclude = transclude_override
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| subnode.transclude.to_string());
        let mut html = format!(
            r#"<wb-subnode identifier="{}" transclude="{}">"#,
            subnode.identifier, transclude
        );
        html.push_str(&format!("<wb-title>{}</wb-title>", subnode.title));
        html.push_str(&render_body(
            &subnode.body,
            transclusion_metadata,
            link_metadata,
            counter,
        ));
        html.push_str("</wb-subnode>");
        html
    }
}

impl Compile for MockFile {
    fn compile(&self, _id: FileId) -> Warned<Result<CompileOutput, EcoVec<SourceDiagnostic>>> {
        let warnings = self
            .warnings
            .iter()
            .map(|message| warning(message))
            .collect::<EcoVec<_>>();

        let output = match &self.mode {
            MockFileMode::CompileError(messages) => Err(messages
                .iter()
                .map(|message| error(message))
                .collect::<EcoVec<_>>()),
            _ => Ok(self.lower_output()),
        };

        Warned { output, warnings }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    CreateFile(NonZeroU16, MockFile),
    UpdateFile(NonZeroU16, FileMutation),
    ReplaceFile(NonZeroU16, MockFile),
    RemoveFile(NonZeroU16),
}

#[derive(Debug, Clone)]
pub enum FileMutation {
    ChangePrimaryTitle(String),
    ChangePrimaryMetadata(Metadata),
    ChangePrimaryBody(Vec<MockElement>),
    RenamePrimary(String),
    AddSubnode(MockSubnode),
    RemoveLastSubnode,
    UpdateFirstSubnode(SubnodeMutation),
    SetWarnings(Vec<String>),
    SetMode(MockFileMode),
}

#[derive(Debug, Clone)]
pub enum SubnodeMutation {
    Rename(String),
    ChangeTitle(String),
    ChangeMetadata(Metadata),
    ChangeBody(Vec<MockElement>),
    ToggleTransclude,
}

pub struct EventConfig {
    pool_size: RangeInclusive<usize>,
    name_pool_size: RangeInclusive<usize>,
    sequence_length: RangeInclusive<usize>,
    batch_count: RangeInclusive<usize>,
    max_subnodes: usize,
    max_body_items: usize,
    max_metadata_keys: usize,
    max_metadata_values: usize,
    max_warnings: usize,
    max_compile_errors: usize,
}

impl EventConfig {
    pub fn from_environment() -> Self {
        Self {
            pool_size: std::env::var("TEST_POOL_SIZE")
                .ok()
                .as_deref()
                .map(parse_size_range)
                .unwrap_or(1..=8),
            name_pool_size: std::env::var("TEST_NAME_POOL_SIZE")
                .ok()
                .as_deref()
                .map(parse_size_range)
                .unwrap_or(1..=10),
            sequence_length: std::env::var("TEST_SEQUENCE_LENGTH")
                .ok()
                .as_deref()
                .map(parse_size_range)
                .unwrap_or(1..=16),
            batch_count: std::env::var("TEST_BATCH_COUNT")
                .ok()
                .as_deref()
                .map(parse_size_range)
                .unwrap_or(1..=12),
            max_subnodes: std::env::var("TEST_MAX_SUBNODES")
                .ok()
                .map(|s| s.parse().expect("TEST_MAX_SUBNODES must be a number"))
                .unwrap_or(2),
            max_body_items: std::env::var("TEST_MAX_BODY_ITEMS")
                .ok()
                .map(|s| s.parse().expect("TEST_MAX_BODY_ITEMS must be a number"))
                .unwrap_or(4),
            max_metadata_keys: std::env::var("TEST_MAX_METADATA_KEYS")
                .ok()
                .map(|s| s.parse().expect("TEST_MAX_METADATA_KEYS must be a number"))
                .unwrap_or(2),
            max_metadata_values: std::env::var("TEST_MAX_METADATA_VALUES")
                .ok()
                .map(|s| {
                    s.parse()
                        .expect("TEST_MAX_METADATA_VALUES must be a number")
                })
                .unwrap_or(2),
            max_warnings: std::env::var("TEST_MAX_WARNINGS")
                .ok()
                .map(|s| s.parse().expect("TEST_MAX_WARNINGS must be a number"))
                .unwrap_or(2),
            max_compile_errors: std::env::var("TEST_MAX_COMPILE_ERRORS")
                .ok()
                .map(|s| s.parse().expect("TEST_MAX_COMPILE_ERRORS must be a number"))
                .unwrap_or(2),
        }
    }
}

#[derive(Debug, Clone)]
enum RawStep {
    Mutate {
        file_id: NonZeroU16,
        created_file: MockFile,
        mutation: RawMutation,
    },
    Replace {
        file_id: NonZeroU16,
        file: MockFile,
    },
    TogglePresence {
        file_id: NonZeroU16,
        file: MockFile,
    },
}

#[derive(Debug, Clone)]
enum RawMutation {
    ChangePrimaryTitle(String),
    ChangePrimaryMetadata(Metadata),
    ChangePrimaryBody(Vec<MockElement>),
    RenamePrimary(String),
    AdjustSubnodes {
        fallback: MockSubnode,
        mutation: Option<SubnodeMutation>,
    },
    SetWarnings(Vec<String>),
    SetMode(MockFileMode),
}

impl RawMutation {
    fn realize(&self, current: &MockFile) -> FileMutation {
        match self {
            RawMutation::ChangePrimaryTitle(title) => {
                FileMutation::ChangePrimaryTitle(title.clone())
            }
            RawMutation::ChangePrimaryMetadata(metadata) => {
                FileMutation::ChangePrimaryMetadata(metadata.clone())
            }
            RawMutation::ChangePrimaryBody(body) => FileMutation::ChangePrimaryBody(body.clone()),
            RawMutation::RenamePrimary(identifier) => {
                FileMutation::RenamePrimary(identifier.clone())
            }
            RawMutation::AdjustSubnodes { fallback, mutation } => {
                if current.subnodes.is_empty() {
                    FileMutation::AddSubnode(fallback.clone())
                } else if let Some(mutation) = mutation {
                    FileMutation::UpdateFirstSubnode(mutation.clone())
                } else {
                    FileMutation::RemoveLastSubnode
                }
            }
            RawMutation::SetWarnings(warnings) => FileMutation::SetWarnings(warnings.clone()),
            RawMutation::SetMode(mode) => FileMutation::SetMode(mode.clone()),
        }
    }
}

pub fn arbitrary_event_batches(config: &EventConfig) -> impl Strategy<Value = Vec<Vec<Event>>> {
    arbitrary_events(config).prop_flat_map(move |events| {
        let length = events.len();
        config
            .batch_count
            .clone()
            .prop_flat_map(move |batch_count| {
                let events = events.clone();
                vec(0..=length, batch_count.saturating_sub(1)).prop_map(move |mut points| {
                    points.sort_unstable();
                    let mut batches = Vec::with_capacity(points.len() + 1);
                    let mut previous = 0;
                    for point in points {
                        batches.push(events[previous..point].to_vec());
                        previous = point;
                    }
                    batches.push(events[previous..].to_vec());
                    batches
                })
            })
    })
}

pub fn arbitrary_events(config: &EventConfig) -> impl Strategy<Value = Vec<Event>> {
    let max_subnodes = config.max_subnodes;
    let max_body_items = config.max_body_items;
    let max_metadata_keys = config.max_metadata_keys;
    let max_metadata_values = config.max_metadata_values;
    let max_warnings = config.max_warnings;
    let max_compile_errors = config.max_compile_errors;
    let length = config.sequence_length.clone();
    let name_pool_size = config.name_pool_size.clone();

    hash_set(any::<NonZeroU16>(), config.pool_size.clone()).prop_flat_map(move |file_ids| {
        let mut file_ids: Vec<_> = file_ids.into_iter().collect();
        file_ids.sort_unstable();

        hash_set(any::<NonZeroU16>(), name_pool_size.clone()).prop_flat_map({
            let length = length.clone();
            move |name_ids| {
                let mut name_ids: Vec<_> = name_ids.into_iter().collect();
                name_ids.sort_unstable();

                let node_names = name_ids
                    .iter()
                    .map(|&id| format_node_name(id))
                    .collect::<Vec<_>>();
                let dangling_names = name_ids
                    .iter()
                    .map(|&id| format!("missing-{}", format_node_name(id)))
                    .collect::<Vec<_>>();
                let mut target_names = node_names.clone();
                target_names.extend(dangling_names);

                let raw_step = arbitrary_raw_step(
                    file_ids.clone(),
                    node_names,
                    target_names,
                    max_subnodes,
                    max_body_items,
                    max_metadata_keys,
                    max_metadata_values,
                    max_warnings,
                    max_compile_errors,
                );

                vec(raw_step, length.clone()).prop_map(realize_steps)
            }
        })
    })
}

fn arbitrary_raw_step(
    file_ids: Vec<NonZeroU16>,
    node_names: Vec<String>,
    target_names: Vec<String>,
    max_subnodes: usize,
    max_body_items: usize,
    max_metadata_keys: usize,
    max_metadata_values: usize,
    max_warnings: usize,
    max_compile_errors: usize,
) -> impl Strategy<Value = RawStep> {
    let file_id = select(file_ids);
    let created_file = arbitrary_mock_file(
        node_names.clone(),
        target_names.clone(),
        max_subnodes,
        max_body_items,
        max_metadata_keys,
        max_metadata_values,
        max_warnings,
        max_compile_errors,
    );
    let replaced_file = arbitrary_mock_file(
        node_names.clone(),
        target_names.clone(),
        max_subnodes,
        max_body_items,
        max_metadata_keys,
        max_metadata_values,
        max_warnings,
        max_compile_errors,
    );
    let toggled_file = arbitrary_mock_file(
        node_names.clone(),
        target_names.clone(),
        max_subnodes,
        max_body_items,
        max_metadata_keys,
        max_metadata_values,
        max_warnings,
        max_compile_errors,
    );
    let mutation = arbitrary_raw_mutation(
        node_names,
        target_names,
        max_body_items,
        max_metadata_keys,
        max_metadata_values,
        max_warnings,
        max_compile_errors,
    );

    prop_oneof![
        5 => (file_id.clone(), created_file, mutation)
            .prop_map(|(file_id, created_file, mutation)| RawStep::Mutate {
                file_id,
                created_file,
                mutation,
            }),
        2 => (file_id.clone(), replaced_file).prop_map(|(file_id, file)| RawStep::Replace {
            file_id,
            file,
        }),
        2 => (file_id, toggled_file).prop_map(|(file_id, file)| RawStep::TogglePresence {
            file_id,
            file,
        }),
    ]
}

fn arbitrary_mock_file(
    node_names: Vec<String>,
    target_names: Vec<String>,
    max_subnodes: usize,
    max_body_items: usize,
    max_metadata_keys: usize,
    max_metadata_values: usize,
    max_warnings: usize,
    max_compile_errors: usize,
) -> impl Strategy<Value = MockFile> {
    let primary = arbitrary_declared_node(
        node_names.clone(),
        target_names.clone(),
        max_body_items,
        max_metadata_keys,
        max_metadata_values,
    );
    let subnode = arbitrary_subnode(
        node_names,
        target_names,
        max_body_items,
        max_metadata_keys,
        max_metadata_values,
    );
    let warnings = arbitrary_messages(max_warnings);
    let mode = arbitrary_file_mode(max_compile_errors);

    (primary, vec(subnode, 0..=max_subnodes), warnings, mode).prop_map(
        |(primary, subnodes, warnings, mode)| MockFile {
            primary,
            subnodes,
            warnings,
            mode,
        },
    )
}

fn arbitrary_declared_node(
    node_names: Vec<String>,
    target_names: Vec<String>,
    max_body_items: usize,
    max_metadata_keys: usize,
    max_metadata_values: usize,
) -> impl Strategy<Value = MockNode> {
    (
        select(node_names),
        "[a-z]{1,8}",
        arbitrary_metadata(max_metadata_keys, max_metadata_values),
        vec(
            arbitrary_inline(target_names, max_metadata_keys, max_metadata_values),
            0..=max_body_items,
        ),
    )
        .prop_map(|(identifier, title, metadata, body)| MockNode {
            identifier,
            title,
            metadata,
            body,
        })
}

fn arbitrary_subnode(
    node_names: Vec<String>,
    target_names: Vec<String>,
    max_body_items: usize,
    max_metadata_keys: usize,
    max_metadata_values: usize,
) -> impl Strategy<Value = MockSubnode> {
    (
        select(node_names),
        "[a-z]{1,8}",
        arbitrary_metadata(max_metadata_keys, max_metadata_values),
        vec(
            arbitrary_inline(target_names, max_metadata_keys, max_metadata_values),
            0..=max_body_items,
        ),
        any::<bool>(),
    )
        .prop_map(
            |(identifier, title, metadata, body, transclude)| MockSubnode {
                identifier,
                title,
                metadata,
                body,
                transclude,
            },
        )
}

fn arbitrary_raw_mutation(
    node_names: Vec<String>,
    target_names: Vec<String>,
    max_body_items: usize,
    max_metadata_keys: usize,
    max_metadata_values: usize,
    max_warnings: usize,
    max_compile_errors: usize,
) -> impl Strategy<Value = RawMutation> {
    let body = vec(
        arbitrary_inline(target_names.clone(), max_metadata_keys, max_metadata_values),
        0..=max_body_items,
    );
    let fallback_subnode = arbitrary_subnode(
        node_names.clone(),
        target_names.clone(),
        max_body_items,
        max_metadata_keys,
        max_metadata_values,
    );
    let subnode_mutation = arbitrary_subnode_mutation(
        node_names.clone(),
        target_names,
        max_body_items,
        max_metadata_keys,
        max_metadata_values,
    );

    prop_oneof![
        2 => "[a-z]{1,8}".prop_map(RawMutation::ChangePrimaryTitle),
        2 => arbitrary_metadata(max_metadata_keys, max_metadata_values)
            .prop_map(RawMutation::ChangePrimaryMetadata),
        2 => body.prop_map(RawMutation::ChangePrimaryBody),
        2 => select(node_names).prop_map(RawMutation::RenamePrimary),
        3 => (fallback_subnode, prop_oneof![1 => Just(None), 3 => subnode_mutation.prop_map(Some)])
            .prop_map(|(fallback, mutation)| RawMutation::AdjustSubnodes { fallback, mutation }),
        1 => arbitrary_messages(max_warnings).prop_map(RawMutation::SetWarnings),
        1 => arbitrary_file_mode(max_compile_errors).prop_map(RawMutation::SetMode),
    ]
}

fn arbitrary_subnode_mutation(
    node_names: Vec<String>,
    target_names: Vec<String>,
    max_body_items: usize,
    max_metadata_keys: usize,
    max_metadata_values: usize,
) -> impl Strategy<Value = SubnodeMutation> {
    let body = vec(
        arbitrary_inline(target_names, max_metadata_keys, max_metadata_values),
        0..=max_body_items,
    );

    prop_oneof![
        1 => select(node_names).prop_map(SubnodeMutation::Rename),
        1 => "[a-z]{1,8}".prop_map(SubnodeMutation::ChangeTitle),
        1 => arbitrary_metadata(max_metadata_keys, max_metadata_values)
            .prop_map(SubnodeMutation::ChangeMetadata),
        1 => body.prop_map(SubnodeMutation::ChangeBody),
        1 => Just(SubnodeMutation::ToggleTransclude),
    ]
}

fn arbitrary_inline(
    target_names: Vec<String>,
    max_metadata_keys: usize,
    max_metadata_values: usize,
) -> impl Strategy<Value = MockElement> {
    prop_oneof![
        3 => "[a-z]{0,8}".prop_map(MockElement::Text),
        2 => (
            select(target_names.clone()),
            "[a-z]{0,8}",
            arbitrary_metadata(max_metadata_keys, max_metadata_values),
        )
            .prop_map(|(target, content, metadata)| MockElement::Link {
                target,
                content,
                metadata,
            }),
        2 => (
            select(target_names),
            arbitrary_metadata(max_metadata_keys, max_metadata_values),
        ).prop_map(|(target, metadata)| {
            MockElement::Transclusion { target, metadata }
        }),
    ]
}

fn arbitrary_metadata(
    max_metadata_keys: usize,
    max_metadata_values: usize,
) -> impl Strategy<Value = Metadata> {
    let values = vec![
        "alpha".to_string(),
        "beta".to_string(),
        "gamma".to_string(),
        "delta".to_string(),
    ];
    let extra_keys = vec!["kind".to_string(), "group".to_string()];
    let tag_values = vec(select(values.clone()), 1..=max_metadata_values.max(1));
    let extras = proptest::collection::hash_map(
        select(extra_keys),
        vec(select(values), 1..=max_metadata_values.max(1)),
        0..=max_metadata_keys.saturating_sub(1),
    );

    (tag_values, extras).prop_map(|(tag, mut extras)| {
        extras.insert("tag".to_string(), tag);
        extras
    })
}

fn arbitrary_messages(max_messages: usize) -> impl Strategy<Value = Vec<String>> {
    vec("[a-z]{1,12}", 0..=max_messages)
}

fn arbitrary_file_mode(max_compile_errors: usize) -> impl Strategy<Value = MockFileMode> {
    prop_oneof![
        6 => Just(MockFileMode::WellFormed),
        1 => Just(MockFileMode::MissingPrimaryNode),
        1 => Just(MockFileMode::DuplicatePrimaryNode),
        1 => Just(MockFileMode::MissingPrimaryTitle),
        1 => Just(MockFileMode::InvalidSubnodeTransclude),
        1 => arbitrary_messages(max_compile_errors).prop_map(MockFileMode::CompileError),
    ]
}

fn realize_steps(steps: Vec<RawStep>) -> Vec<Event> {
    let mut current: HashMap<NonZeroU16, MockFile> = HashMap::new();
    let mut events = Vec::with_capacity(steps.len());

    for step in steps {
        match step {
            RawStep::Mutate {
                file_id,
                created_file,
                mutation,
            } => {
                if let Some(existing) = current.get_mut(&file_id) {
                    let realized = mutation.realize(existing);
                    existing.apply_mutation(&realized);
                    events.push(Event::UpdateFile(file_id, realized));
                } else {
                    current.insert(file_id, created_file.clone());
                    events.push(Event::CreateFile(file_id, created_file));
                }
            }
            RawStep::Replace { file_id, file } => {
                let event = if current.contains_key(&file_id) {
                    Event::ReplaceFile(file_id, file.clone())
                } else {
                    Event::CreateFile(file_id, file.clone())
                };
                current.insert(file_id, file);
                events.push(event);
            }
            RawStep::TogglePresence { file_id, file } => {
                if current.remove(&file_id).is_some() {
                    events.push(Event::RemoveFile(file_id));
                } else {
                    current.insert(file_id, file.clone());
                    events.push(Event::CreateFile(file_id, file));
                }
            }
        }
    }

    events
}

fn render_body(
    body: &[MockElement],
    transclusion_metadata: &mut HashMap<u32, Metadata>,
    link_metadata: &mut HashMap<u32, Metadata>,
    counter: &mut u32,
) -> String {
    let mut html = String::new();

    for inline in body {
        match inline {
            MockElement::Text(text) => {
                html.push_str(&format!("<p>{text}</p>"));
            }
            MockElement::Link {
                target,
                content,
                metadata,
            } => {
                let current = *counter;
                *counter += 1;
                link_metadata.insert(current, metadata.clone());
                html.push_str(&format!(
                    r#"<a href="wb:{target}" data-counter="{current}">{content}</a>"#
                ));
            }
            MockElement::Transclusion { target, metadata } => {
                let current = *counter;
                *counter += 1;
                transclusion_metadata.insert(current, metadata.clone());
                html.push_str(&format!(
                    r#"<wb-transclude identifier="{target}" counter="{current}"></wb-transclude>"#
                ));
            }
        }
    }

    html
}

fn warning(message: &str) -> SourceDiagnostic {
    SourceDiagnostic::warning(Span::detached(), EcoString::from(message))
}

fn error(message: &str) -> SourceDiagnostic {
    SourceDiagnostic::error(Span::detached(), EcoString::from(message))
}

fn parse_size_range(s: &str) -> RangeInclusive<usize> {
    if let Some((lo, hi)) = s.split_once("..") {
        let lo = lo.parse().expect("invalid lower bound in size range");
        let hi = hi.parse().expect("invalid upper bound in size range");
        lo..=hi
    } else {
        let n = s
            .parse()
            .expect("TEST_* size value must be a number or range");
        n..=n
    }
}

fn format_node_name(id: NonZeroU16) -> String {
    format!("n{id}")
}

fn unique_identifier(candidate: &str, used: &mut HashSet<String>, fallback: &str) -> String {
    let base = if candidate.is_empty() {
        fallback
    } else {
        candidate
    };
    let mut identifier = base.to_string();
    let mut suffix = 1usize;

    while used.contains(&identifier) {
        identifier = format!("{base}-{suffix}");
        suffix += 1;
    }

    used.insert(identifier.clone());
    identifier
}
