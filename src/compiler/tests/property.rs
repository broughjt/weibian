use std::collections::HashMap;

use proptest::prelude::*;
use proptest_state_machine::{ReferenceStateMachine, StateMachineTest, prop_state_machine};
use typst::syntax::Span;

use crate::compiler::{Compiler, Metadata};
use crate::config::RenderConfig;

use super::mock::{AbstractState, MockLink, MockNode, MockTransclusion, Transition};
use super::stateless::process_stateless;

fn test_render_config() -> RenderConfig {
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
        "[A-Za-z ]{1,12}",   // title
        "[a-z ]{0,20}",      // body
        metadata_strategy(), // node metadata
        proptest::collection::vec(
            // transclusions
            (
                proptest::sample::select(targets_for_transclusions),
                metadata_strategy(),
            )
                .prop_map(|(target, metadata)| MockTransclusion { target, metadata }),
            0..=3,
        ),
        proptest::collection::vec(
            // links
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

        // CreateFile — always available, generates 1-3 nodes
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

        // RemoveFile
        if !file_ids.is_empty() {
            let file_ids_clone = file_ids.clone();
            strategies.push(
                proptest::sample::select(file_ids_clone)
                    .prop_map(|file_id| Transition::RemoveFile { file_id })
                    .boxed(),
            );
        }

        // AddNode
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

        // RemoveNode — only from files with 2+ nodes so we don't empty a file
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

        // AddTransclusion
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

        // RemoveTransclusion
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

        // AddLink
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

        // RemoveLink
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

        // UpdateTitle
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

        // UpdateBody
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

        // UpdateMetadata
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
        // Update counters for new IDs before applying
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

// ---------------------------------------------------------------------------
// StateMachineTest
// ---------------------------------------------------------------------------

struct CompilerStateMachineTest;

impl StateMachineTest for CompilerStateMachineTest {
    type SystemUnderTest = Compiler;
    type Reference = AbstractState;

    fn init_test(
        _ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) -> Self::SystemUnderTest {
        Compiler::default()
    }

    fn apply(
        mut state: Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
        transition: <Self::Reference as ReferenceStateMachine>::Transition,
    ) -> Self::SystemUnderTest {
        let file_id_raw = ref_state.affected_file_id(&transition);

        match &transition {
            Transition::RemoveFile { .. } => {
                let typst_id = AbstractState::to_typst_file_id(file_id_raw);
                state.remove(typst_id);
            }
            _ => {
                // All other transitions modify a file — recompile it.
                // Note: ref_state already has the transition applied (by
                // ReferenceStateMachine::apply), so compile_file gives us
                // the post-transition state.
                let typst_id = AbstractState::to_typst_file_id(file_id_raw);
                let compiled = ref_state.compile_file(file_id_raw);
                state._update(typst_id, compiled);
            }
        }

        state
    }

    fn check_invariants(
        state: &Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) {
        let config = test_render_config();

        // Clone the compiler so process() doesn't mutate the real one
        let mut compiler_clone: Compiler = state.clone();
        let incremental_output = compiler_clone
            .process(&config)
            .expect("incremental process() failed");

        let (stateless_output, _stateless_compile_diags, _stateless_process_diags) =
            process_stateless(ref_state, &config).expect("stateless process() failed");

        // The key property: rendered HTML output should be identical
        assert_eq!(
            incremental_output.writes, stateless_output,
            "incremental output differs from stateless reference"
        );
    }
}

// ---------------------------------------------------------------------------
// Test entry point
// ---------------------------------------------------------------------------

prop_state_machine! {
    #[test]
    fn incremental_matches_stateless(sequential 1..20 => CompilerStateMachineTest);
}
