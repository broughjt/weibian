mod config;
mod driver;
mod model;
mod process_stateless;
mod reference_compiler;
mod render;

use std::{
    collections::HashMap,
    num::NonZeroU16,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use ecow::EcoVec;
use proptest::{
    collection::vec,
    prelude::{Just, Strategy, proptest},
};
use proptest_state_machine::{ReferenceStateMachine, StateMachineTest, prop_state_machine};
use typst::diag::{Severity, SourceDiagnostic};
use typst_syntax::Span;

use crate::compiler::{
    CompileDiagnostics, Compiler, OutputPlan, ProcessDiagnostics,
    tests::{
        config::CONFIG,
        driver::run_sequential,
        model::{MockNode, MockNodeIdentifier},
        process_stateless::process_stateless,
        reference_compiler::{MockFile, MockNodeId, ReferenceCompiler, State, Transition, file_id},
        render::{MockRenderer, RenderBackmatter, RenderBody, RenderNode},
    },
};

struct IncrementalMatchesStateless;

#[derive(Default)]
struct IncrementalCompiler {
    compiler: Compiler<RenderBody, RenderBackmatter>,
    filesystem: HashMap<String, RenderNode>,
}

impl StateMachineTest for IncrementalMatchesStateless {
    type SystemUnderTest = IncrementalCompiler;
    type Reference = ReferenceCompiler;

    fn init_test(
        _ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) -> Self::SystemUnderTest {
        IncrementalCompiler::default()
    }

    fn apply(
        mut state: Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
        transition: <Self::Reference as ReferenceStateMachine>::Transition,
    ) -> Self::SystemUnderTest {
        let file_id = transition.file_id();

        if matches!(transition, Transition::RemoveFile(_)) {
            state.compiler.remove(file_id);
        } else {
            // TODO: Write a test for this
            state
                .compiler
                ._update(file_id, ref_state.compile_file(file_id));
        }

        let plan = state
            .compiler
            ._process(&MockRenderer)
            .expect("bug: MockRenderer cannot fail");
        apply_plan(plan, &mut state.filesystem);

        state
    }

    fn check_invariants(
        state: &Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) {
        assert_matches_stateless(state, ref_state);
    }
}

struct IncrementalMatchesStatelessBatched;

impl StateMachineTest for IncrementalMatchesStatelessBatched {
    type SystemUnderTest = IncrementalCompiler;
    type Reference = ReferenceCompiler;

    fn init_test(
        _ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) -> Self::SystemUnderTest {
        IncrementalCompiler::default()
    }

    fn apply(
        mut state: Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
        transition: <Self::Reference as ReferenceStateMachine>::Transition,
    ) -> Self::SystemUnderTest {
        let file_id = transition.file_id();

        if matches!(transition, Transition::RemoveFile(_)) {
            state.compiler.remove(file_id);
        } else {
            state
                .compiler
                ._update(file_id, ref_state.compile_file(file_id));
        }

        state
    }
}

/// Custom runner mirroring `proptest_state_machine::test_sequential` but
/// processing only at batch boundaries.
fn run_batched(
    initial_state: State,
    transitions: Vec<Transition>,
    mut seen_counter: Option<Arc<AtomicUsize>>,
    batch_sizes: Vec<usize>,
) {
    let batches = {
        let mut transitions = transitions.into_iter();
        batch_sizes
            .into_iter()
            .map(|size| transitions.by_ref().take(size).collect::<Vec<_>>())
            .take_while(|batch| !batch.is_empty())
            .collect::<Vec<Vec<Transition>>>()
    };

    let mut ref_state = initial_state;
    let mut sut = IncrementalMatchesStatelessBatched::init_test(&ref_state);

    assert_matches_stateless(&sut, &ref_state);

    for batch in batches {
        for transition in batch {
            if let Some(counter) = seen_counter.as_mut() {
                counter.fetch_add(1, Ordering::SeqCst);
            }
            ref_state = ReferenceCompiler::apply(ref_state, &transition);
            sut = IncrementalMatchesStatelessBatched::apply(sut, &ref_state, transition);
        }
        let plan = sut
            .compiler
            ._process(&MockRenderer)
            .expect("bug: MockRenderer cannot fail");
        apply_plan(plan, &mut sut.filesystem);
        assert_matches_stateless(&sut, &ref_state);
    }
}

/// Output of `ReferenceCompiler::sequential_strategy`: initial state, the
/// generated transition sequence, and proptest's per-step seen-counter (used
/// during shrinking to mark transitions that were actually applied).
type StateMachineInput = (State, Vec<Transition>, Option<Arc<AtomicUsize>>);

/// Strategy producing batched test inputs: the underlying state-machine
/// strategy paired with a batch-size schedule of `batch_sizes_strategy(n)`,
/// where `n` is the realized transition count.
fn batched_strategy<S>(
    batch_sizes_strategy: impl Fn(usize) -> S,
) -> impl Strategy<Value = (StateMachineInput, Vec<usize>)>
where
    S: Strategy<Value = Vec<usize>>,
{
    ReferenceCompiler::sequential_strategy(CONFIG.transitions.clone()).prop_flat_map(move |u| {
        let n = u.1.len();
        (Just(u), batch_sizes_strategy(n))
    })
}

fn assert_matches_stateless(incremental: &IncrementalCompiler, state: &State) {
    let (expected_output, expected_compile_diagnostics, expected_process_diagnostics) =
        process_stateless(state).expect("stateless reference must succeed");

    assert_eq!(incremental.filesystem, expected_output);
    assert_eq!(
        normalize_compile_diagnostics(incremental.compiler.compile_diagnostics()),
        normalize_compile_diagnostics(&expected_compile_diagnostics),
    );
    assert_eq!(
        normalize_process_diagnostics(incremental.compiler.process_diagnostics()),
        normalize_process_diagnostics(&expected_process_diagnostics),
    );
}

fn apply_plan(plan: OutputPlan<RenderNode>, filesystem: &mut HashMap<String, RenderNode>) {
    // TODO: Could test that inserts and removes are always disjoint in both the reference compiler and the incremental compiler
    for (name, node) in plan.writes {
        filesystem.insert(name, node);
    }
    for name in plan.deletes {
        filesystem.remove(&name);
    }
}

fn diagnostic_sort_key(d: &SourceDiagnostic) -> (u8, &ecow::EcoString, std::num::NonZeroU64) {
    let severity = match d.severity {
        Severity::Error => 0,
        Severity::Warning => 1,
    };
    (severity, &d.message, d.span.into_raw())
}

fn sorted(diagnostics: &EcoVec<SourceDiagnostic>) -> EcoVec<SourceDiagnostic> {
    let mut v: Vec<SourceDiagnostic> = diagnostics.iter().cloned().collect();
    v.sort_by(|a, b| diagnostic_sort_key(a).cmp(&diagnostic_sort_key(b)));
    v.into()
}

fn normalize_compile_diagnostics(d: &CompileDiagnostics) -> CompileDiagnostics {
    d.iter()
        .map(|(&f, (warnings, errors))| (f, (sorted(warnings), sorted(errors))))
        .collect()
}

fn normalize_process_diagnostics(d: &ProcessDiagnostics) -> ProcessDiagnostics {
    d.iter()
        .map(|(&f, diagnostics)| (f, sorted(diagnostics)))
        .collect()
}

prop_state_machine! {
    #[test]
    fn incremental_matches_stateless(sequential CONFIG.transitions.clone() => IncrementalMatchesStateless);
}

proptest! {
    #[test]
    fn incremental_matches_stateless2(
        (initial_state, transitions, seen_counter) in
            ReferenceCompiler::sequential_strategy(CONFIG.transitions.clone())
    ) {
        run_sequential::<IncrementalMatchesStateless>(
            initial_state,
            transitions,
            seen_counter,
        );
    }

    #[test]
    fn incremental_matches_stateless_batched_uniform(
        ((initial_state, transitions, seen_counter), batch_sizes) in batched_strategy(|n| {
            vec(CONFIG.batch.clone(), n)
        })
    ) {
        run_batched(initial_state, transitions, seen_counter, batch_sizes);
    }

    #[test]
    fn incremental_matches_stateless_batched_max(
        ((initial_state, transitions, seen_counter), batch_sizes) in batched_strategy(|n| {
            Just(vec![*CONFIG.batch.end(); n])
        })
    ) {
        run_batched(initial_state, transitions, seen_counter, batch_sizes);
    }

    #[test]
    fn incremental_matches_stateless_batched_skewed_large(
        ((initial_state, transitions, seen_counter), batch_sizes) in batched_strategy(|n| {
            vec(
                (CONFIG.batch.clone(), CONFIG.batch.clone()).prop_map(|(a, b)| a.max(b)),
                n,
            )
        })
    ) {
        run_batched(initial_state, transitions, seen_counter, batch_sizes);
    }
}

#[test]
fn duplicate_identifier_after_compile_error_recovery_matches_stateless() {
    let mut reference = State::default();
    let mut incremental = IncrementalCompiler::default();

    let file1 = file_id(NonZeroU16::new(1).unwrap());
    reference.insert_file(
        file1,
        MockFile {
            nodes: [(
                MockNodeId(0),
                MockNode {
                    identifier: MockNodeIdentifier(0),
                    title: Default::default(),
                    body: Default::default(),
                    span: Span::detached(),
                    metadata: Default::default(),
                    transclusions: Default::default(),
                    links: Default::default(),
                },
            )]
            .into_iter()
            .collect(),
            errors: vec!["compile error".into()],
            warnings: vec!["compile warning".into()],
        },
    );
    incremental
        .compiler
        ._update(file1, reference.compile_file(file1));
    let plan = incremental.compiler._process(&MockRenderer).unwrap();
    apply_plan(plan, &mut incremental.filesystem);
    assert_matches_stateless(&incremental, &reference);

    let file2 = file_id(NonZeroU16::new(2).unwrap());
    reference.insert_file(
        file2,
        MockFile {
            nodes: [(
                MockNodeId(1),
                MockNode {
                    identifier: MockNodeIdentifier(0),
                    title: Default::default(),
                    body: Default::default(),
                    span: Span::detached(),
                    metadata: Default::default(),
                    transclusions: Default::default(),
                    links: Default::default(),
                },
            )]
            .into_iter()
            .collect(),
            errors: Vec::new(),
            warnings: Vec::new(),
        },
    );
    incremental
        .compiler
        ._update(file2, reference.compile_file(file2));
    let plan = incremental.compiler._process(&MockRenderer).unwrap();
    apply_plan(plan, &mut incremental.filesystem);
    assert_matches_stateless(&incremental, &reference);

    reference.files.get_mut(&file1).unwrap().errors.clear();
    incremental
        .compiler
        ._update(file1, reference.compile_file(file1));
    let plan = incremental.compiler._process(&MockRenderer).unwrap();
    apply_plan(plan, &mut incremental.filesystem);
    assert_matches_stateless(&incremental, &reference);
}

#[test]
fn duplicate_identifier_resolves_when_other_file_is_deleted() {
    let mut reference = State::default();
    let mut incremental = IncrementalCompiler::default();

    let file1 = file_id(NonZeroU16::new(1).unwrap());
    reference.insert_file(
        file1,
        MockFile {
            nodes: [(
                MockNodeId(0),
                MockNode {
                    identifier: MockNodeIdentifier(0),
                    title: Default::default(),
                    body: Default::default(),
                    span: Span::detached(),
                    metadata: Default::default(),
                    transclusions: Default::default(),
                    links: Default::default(),
                },
            )]
            .into_iter()
            .collect(),
            errors: Vec::new(),
            warnings: Vec::new(),
        },
    );
    incremental
        .compiler
        ._update(file1, reference.compile_file(file1));
    let plan = incremental.compiler._process(&MockRenderer).unwrap();
    apply_plan(plan, &mut incremental.filesystem);
    assert_matches_stateless(&incremental, &reference);

    let file2 = file_id(NonZeroU16::new(2).unwrap());
    reference.insert_file(
        file2,
        MockFile {
            nodes: [(
                MockNodeId(1),
                MockNode {
                    identifier: MockNodeIdentifier(0),
                    title: Default::default(),
                    body: Default::default(),
                    span: Span::detached(),
                    metadata: Default::default(),
                    transclusions: Default::default(),
                    links: Default::default(),
                },
            )]
            .into_iter()
            .collect(),
            errors: Vec::new(),
            warnings: Vec::new(),
        },
    );
    incremental
        .compiler
        ._update(file2, reference.compile_file(file2));
    let plan = incremental.compiler._process(&MockRenderer).unwrap();
    apply_plan(plan, &mut incremental.filesystem);
    assert_matches_stateless(&incremental, &reference);

    reference.remove_file(file1);
    incremental.compiler.remove(file1);
    let plan = incremental.compiler._process(&MockRenderer).unwrap();
    apply_plan(plan, &mut incremental.filesystem);
    assert_matches_stateless(&incremental, &reference);
}

#[test]
fn duplicate_identifier_resolves_when_other_file_regains_compile_error() {
    let mut reference = State::default();
    let mut incremental = IncrementalCompiler::default();

    let file1 = file_id(NonZeroU16::new(1).unwrap());
    reference.insert_file(
        file1,
        MockFile {
            nodes: [(
                MockNodeId(0),
                MockNode {
                    identifier: MockNodeIdentifier(0),
                    title: Default::default(),
                    body: Default::default(),
                    span: Span::detached(),
                    metadata: Default::default(),
                    transclusions: Default::default(),
                    links: Default::default(),
                },
            )]
            .into_iter()
            .collect(),
            errors: Vec::new(),
            warnings: Vec::new(),
        },
    );
    incremental
        .compiler
        ._update(file1, reference.compile_file(file1));
    let plan = incremental.compiler._process(&MockRenderer).unwrap();
    apply_plan(plan, &mut incremental.filesystem);
    assert_matches_stateless(&incremental, &reference);

    let file2 = file_id(NonZeroU16::new(2).unwrap());
    reference.insert_file(
        file2,
        MockFile {
            nodes: [(
                MockNodeId(1),
                MockNode {
                    identifier: MockNodeIdentifier(0),
                    title: Default::default(),
                    body: Default::default(),
                    span: Span::detached(),
                    metadata: Default::default(),
                    transclusions: Default::default(),
                    links: Default::default(),
                },
            )]
            .into_iter()
            .collect(),
            errors: Vec::new(),
            warnings: Vec::new(),
        },
    );
    incremental
        .compiler
        ._update(file2, reference.compile_file(file2));
    let plan = incremental.compiler._process(&MockRenderer).unwrap();
    apply_plan(plan, &mut incremental.filesystem);
    assert_matches_stateless(&incremental, &reference);

    reference
        .files
        .get_mut(&file1)
        .unwrap()
        .errors
        .push("compile error".into());
    incremental
        .compiler
        ._update(file1, reference.compile_file(file1));
    let plan = incremental.compiler._process(&MockRenderer).unwrap();
    apply_plan(plan, &mut incremental.filesystem);
    assert_matches_stateless(&incremental, &reference);
}

#[test]
fn duplicate_identifier_third_file_gets_diagnostic() {
    let mut reference = State::default();
    let mut incremental = IncrementalCompiler::default();

    // Each file contributes a single node with the same identifier
    // (MockNodeIdentifier(0)).
    let mock_file = |node_id: MockNodeId| MockFile {
        nodes: [(
            node_id,
            MockNode {
                identifier: MockNodeIdentifier(0),
                title: Default::default(),
                body: Default::default(),
                span: Span::detached(),
                metadata: Default::default(),
                transclusions: Default::default(),
                links: Default::default(),
            },
        )]
        .into_iter()
        .collect(),
        errors: Vec::new(),
        warnings: Vec::new(),
    };

    let file1 = file_id(NonZeroU16::new(1).unwrap());
    reference.insert_file(file1, mock_file(MockNodeId(0)));
    incremental
        .compiler
        ._update(file1, reference.compile_file(file1));
    let plan = incremental.compiler._process(&MockRenderer).unwrap();
    apply_plan(plan, &mut incremental.filesystem);
    assert_matches_stateless(&incremental, &reference);

    let file2 = file_id(NonZeroU16::new(2).unwrap());
    reference.insert_file(file2, mock_file(MockNodeId(1)));
    incremental
        .compiler
        ._update(file2, reference.compile_file(file2));
    let plan = incremental.compiler._process(&MockRenderer).unwrap();
    apply_plan(plan, &mut incremental.filesystem);
    assert_matches_stateless(&incremental, &reference);

    // file_3 also claims the identifier. The identifier was already
    // duplicated and stays duplicated — its canonical state did not
    // change. If `_process` early-returns because nothing landed in
    // dirty/removed, the cached process_diagnostics from the previous
    // call will only mention file_1 and file_2; file_3's duplicate
    // diagnostic will be missing.
    let file3 = file_id(NonZeroU16::new(3).unwrap());
    reference.insert_file(file3, mock_file(MockNodeId(2)));
    incremental
        .compiler
        ._update(file3, reference.compile_file(file3));
    let plan = incremental.compiler._process(&MockRenderer).unwrap();
    apply_plan(plan, &mut incremental.filesystem);
    assert_matches_stateless(&incremental, &reference);
}
