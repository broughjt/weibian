mod config;
mod driver;
mod model;
mod process_stateless;
mod reference_compiler;
mod render;

use std::{
    collections::{HashMap, HashSet},
    num::NonZeroU16,
};

use ecow::EcoVec;
use proptest::{
    collection::vec,
    prelude::{Just, Strategy, proptest},
};
use proptest_state_machine::{ReferenceStateMachine, StateMachineTest};
use typst::diag::{Severity, SourceDiagnostic};
use typst_syntax::Span;

use crate::compiler::{
    CompileDiagnostics, Compiler, OutputPlan, ProcessDiagnostics,
    tests::{
        config::CONFIG,
        driver::{
            StateMachineTestBatched, StateMachineTestCompared, batched_strategy, run_batched,
            run_compared, run_sequential,
        },
        model::{MockNode, MockNodeIdentifier},
        process_stateless::process_stateless,
        reference_compiler::{MockFile, MockNodeId, ReferenceCompiler, State, Transition, file_id},
        render::{MockRenderer, RenderBackmatter, RenderBody, RenderNode},
    },
};

proptest! {
    #[test]
    fn incremental_matches_stateless(
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
    fn output_plan_deletes_match_stateless_difference(
        (initial_state, transitions, seen_counter) in
            ReferenceCompiler::sequential_strategy(CONFIG.transitions.clone())
    ) {
        run_compared::<OutputPlanDeletesMatchStatelessDifference>(
            initial_state,
            transitions,
            seen_counter,
        );
    }

    #[test]
    fn output_plan_writes_changed_outputs(
        (initial_state, transitions, seen_counter) in
            ReferenceCompiler::sequential_strategy(CONFIG.transitions.clone())
    ) {
        run_compared::<OutputPlanWritesChangedOutputs>(
            initial_state,
            transitions,
            seen_counter,
        );
    }

    #[test]
    fn output_plan_writes_and_deletes_are_disjoint(
        (initial_state, transitions, seen_counter) in
            ReferenceCompiler::sequential_strategy(CONFIG.transitions.clone())
    ) {
        run_sequential::<OutputPlanWritesAndDeletesAreDisjoint>(
            initial_state,
            transitions,
            seen_counter,
        );
    }

    #[test]
    fn incremental_matches_stateless_batched_uniform(
        ((initial_state, transitions, seen_counter), batch_sizes) in
            batched_strategy::<IncrementalMatchesStatelessBatched, _>(CONFIG.transitions.clone(), |n| {
                vec(CONFIG.batch.clone(), n)
            })
    ) {
        run_batched::<IncrementalMatchesStatelessBatched>(initial_state, transitions, seen_counter, batch_sizes);
    }

    #[test]
    fn incremental_matches_stateless_batched_max(
        ((initial_state, transitions, seen_counter), batch_sizes) in
            batched_strategy::<IncrementalMatchesStatelessBatched, _>(CONFIG.transitions.clone(), |n| {
                Just(vec![*CONFIG.batch.end(); n])
            })
    ) {
        run_batched::<IncrementalMatchesStatelessBatched>(initial_state, transitions, seen_counter, batch_sizes);
    }

    #[test]
    fn incremental_matches_stateless_batched_skewed_large(
        ((initial_state, transitions, seen_counter), batch_sizes) in
            batched_strategy::<IncrementalMatchesStatelessBatched, _>(CONFIG.transitions.clone(), |n| {
                vec(
                    (CONFIG.batch.clone(), CONFIG.batch.clone()).prop_map(|(a, b)| a.max(b)),
                    n,
                )
            })
    ) {
        run_batched::<IncrementalMatchesStatelessBatched>(initial_state, transitions, seen_counter, batch_sizes);
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
    incremental.apply_plan(plan);
    incremental.assert_matches_stateless(&reference);

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
    incremental.apply_plan(plan);
    incremental.assert_matches_stateless(&reference);

    reference.files.get_mut(&file1).unwrap().errors.clear();
    incremental
        .compiler
        ._update(file1, reference.compile_file(file1));
    let plan = incremental.compiler._process(&MockRenderer).unwrap();
    incremental.apply_plan(plan);
    incremental.assert_matches_stateless(&reference);
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
    incremental.apply_plan(plan);
    incremental.assert_matches_stateless(&reference);

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
    incremental.apply_plan(plan);
    incremental.assert_matches_stateless(&reference);

    reference.remove_file(file1);
    incremental.compiler.remove(file1);
    let plan = incremental.compiler._process(&MockRenderer).unwrap();
    incremental.apply_plan(plan);
    incremental.assert_matches_stateless(&reference);
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
    incremental.apply_plan(plan);
    incremental.assert_matches_stateless(&reference);

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
    incremental.apply_plan(plan);
    incremental.assert_matches_stateless(&reference);

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
    incremental.apply_plan(plan);
    incremental.assert_matches_stateless(&reference);
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
    incremental.apply_plan(plan);
    incremental.assert_matches_stateless(&reference);

    let file2 = file_id(NonZeroU16::new(2).unwrap());
    reference.insert_file(file2, mock_file(MockNodeId(1)));
    incremental
        .compiler
        ._update(file2, reference.compile_file(file2));
    let plan = incremental.compiler._process(&MockRenderer).unwrap();
    incremental.apply_plan(plan);
    incremental.assert_matches_stateless(&reference);

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
    incremental.apply_plan(plan);
    incremental.assert_matches_stateless(&reference);
}

struct IncrementalMatchesStateless;

#[derive(Default)]
struct IncrementalCompiler {
    compiler: Compiler<RenderBody, RenderBackmatter>,
    filesystem: HashMap<String, RenderNode>,
}

impl IncrementalCompiler {
    fn apply_transition(&mut self, specification_state: &State, transition: &Transition) {
        let file_id = transition.file_id();

        if matches!(transition, Transition::RemoveFile(_)) {
            self.compiler.remove(file_id);
        } else {
            self.compiler
                ._update(file_id, specification_state.compile_file(file_id));
        }
    }

    fn apply_plan(&mut self, plan: OutputPlan<RenderNode>) {
        // TODO: Could test that inserts and removes are always disjoint in both the reference compiler and the incremental compiler
        for (name, node) in plan.writes {
            self.filesystem.insert(name, node);
        }
        for name in plan.deletes {
            self.filesystem.remove(&name);
        }
    }

    fn assert_matches_stateless(&self, state: &State) {
        let (expected_output, expected_compile_diagnostics, expected_process_diagnostics) =
            process_stateless(state).expect("stateless reference must succeed");

        assert_eq!(self.filesystem, expected_output);
        assert_eq!(
            normalize_compile_diagnostics(self.compiler.compile_diagnostics()),
            normalize_compile_diagnostics(&expected_compile_diagnostics),
        );
        assert_eq!(
            normalize_process_diagnostics(self.compiler.process_diagnostics()),
            normalize_process_diagnostics(&expected_process_diagnostics),
        );
    }
}

impl StateMachineTest for IncrementalMatchesStateless {
    type SystemUnderTest = IncrementalCompiler;
    type Reference = ReferenceCompiler;

    fn init_test(
        _specification_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) -> Self::SystemUnderTest {
        IncrementalCompiler::default()
    }

    fn apply(
        mut implementation_state: Self::SystemUnderTest,
        specification_state: &<Self::Reference as ReferenceStateMachine>::State,
        transition: <Self::Reference as ReferenceStateMachine>::Transition,
    ) -> Self::SystemUnderTest {
        implementation_state.apply_transition(specification_state, &transition);

        let plan = implementation_state
            .compiler
            ._process(&MockRenderer)
            .expect("bug: MockRenderer cannot fail");
        implementation_state.apply_plan(plan);

        implementation_state
    }

    fn check_invariants(
        implementation_state: &Self::SystemUnderTest,
        specification_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) {
        implementation_state.assert_matches_stateless(specification_state);
    }
}

struct IncrementalMatchesStatelessBatched;

impl StateMachineTest for IncrementalMatchesStatelessBatched {
    type SystemUnderTest = IncrementalCompiler;
    type Reference = ReferenceCompiler;

    fn init_test(
        _specification_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) -> Self::SystemUnderTest {
        IncrementalCompiler::default()
    }

    fn apply(
        mut implementation_state: Self::SystemUnderTest,
        specification_state: &<Self::Reference as ReferenceStateMachine>::State,
        transition: <Self::Reference as ReferenceStateMachine>::Transition,
    ) -> Self::SystemUnderTest {
        implementation_state.apply_transition(specification_state, &transition);

        implementation_state
    }

    fn check_invariants(
        implementation_state: &Self::SystemUnderTest,
        specification_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) {
        implementation_state.assert_matches_stateless(specification_state);
    }
}

impl StateMachineTestBatched for IncrementalMatchesStatelessBatched {
    fn apply_batch(
        mut implementation_state: Self::SystemUnderTest,
        _specification_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) -> Self::SystemUnderTest {
        let plan = implementation_state
            .compiler
            ._process(&MockRenderer)
            .expect("bug: MockRenderer cannot fail");
        implementation_state.apply_plan(plan);

        implementation_state
    }
}

struct OutputPlanDeletesMatchStatelessDifference;

impl StateMachineTestCompared for OutputPlanDeletesMatchStatelessDifference {
    type Implementation = IncrementalCompiler;
    type Specification = ReferenceCompiler;

    fn new(
        _specification_state: &<Self::Specification as ReferenceStateMachine>::State,
    ) -> Self::Implementation {
        IncrementalCompiler::default()
    }

    fn apply(
        mut implementation_state: Self::Implementation,
        specification_state_before: &<Self::Specification as ReferenceStateMachine>::State,
        specification_state_after: &<Self::Specification as ReferenceStateMachine>::State,
        transition: <Self::Specification as ReferenceStateMachine>::Transition,
    ) -> Self::Implementation {
        implementation_state.apply_transition(specification_state_after, &transition);

        let plan = implementation_state
            .compiler
            ._process(&MockRenderer)
            .expect("bug: MockRenderer cannot fail");
        let (output_before, _, _) = process_stateless(specification_state_before)
            .expect("stateless reference must succeed");
        let (output_after, _, _) =
            process_stateless(specification_state_after).expect("stateless reference must succeed");
        let keys_before: HashSet<String> = output_before.keys().cloned().collect();
        let keys_after: HashSet<String> = output_after.keys().cloned().collect();
        let expected_deletes = &keys_before - &keys_after;

        assert_eq!(
            plan.deletes, expected_deletes,
            "output plan deletes should be exactly the outputs present before the transition and absent after it; transition: {transition:?}"
        );

        implementation_state.apply_plan(plan);

        implementation_state
    }
}

struct OutputPlanWritesChangedOutputs;

impl StateMachineTestCompared for OutputPlanWritesChangedOutputs {
    type Implementation = IncrementalCompiler;
    type Specification = ReferenceCompiler;

    fn new(
        _specification_state: &<Self::Specification as ReferenceStateMachine>::State,
    ) -> Self::Implementation {
        IncrementalCompiler::default()
    }

    fn apply(
        mut implementation_state: Self::Implementation,
        specification_state_before: &<Self::Specification as ReferenceStateMachine>::State,
        specification_state_after: &<Self::Specification as ReferenceStateMachine>::State,
        transition: <Self::Specification as ReferenceStateMachine>::Transition,
    ) -> Self::Implementation {
        implementation_state.apply_transition(specification_state_after, &transition);

        let plan = implementation_state
            .compiler
            ._process(&MockRenderer)
            .expect("bug: MockRenderer cannot fail");
        let (output_before, _, _) = process_stateless(specification_state_before)
            .expect("stateless reference must succeed");
        let (output_after, _, _) =
            process_stateless(specification_state_after).expect("stateless reference must succeed");
        // TODO: What's going on with the `name` thing?
        let expected_writes: HashSet<String> = output_after
            .iter()
            .filter(|(name, node)| output_before.get(*name) != Some(*node))
            .map(|(name, _)| name.clone())
            .collect();
        let actual_writes: HashSet<String> = plan.writes.keys().cloned().collect();
        let mut missing: Vec<_> = expected_writes
            .difference(&actual_writes)
            .cloned()
            .collect();
        missing.sort();

        assert!(
            missing.is_empty(),
            "output plan omitted writes for changed outputs: {missing:?}; transition: {transition:?}"
        );

        implementation_state.apply_plan(plan);

        implementation_state
    }
}

struct OutputPlanWritesAndDeletesAreDisjoint;

impl StateMachineTest for OutputPlanWritesAndDeletesAreDisjoint {
    type SystemUnderTest = IncrementalCompiler;
    type Reference = ReferenceCompiler;

    fn init_test(
        _specification_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) -> Self::SystemUnderTest {
        IncrementalCompiler::default()
    }

    fn apply(
        mut implementation_state: Self::SystemUnderTest,
        specification_state: &<Self::Reference as ReferenceStateMachine>::State,
        transition: <Self::Reference as ReferenceStateMachine>::Transition,
    ) -> Self::SystemUnderTest {
        implementation_state.apply_transition(specification_state, &transition);

        let plan = implementation_state
            .compiler
            ._process(&MockRenderer)
            .expect("bug: MockRenderer cannot fail");
        let write_keys: HashSet<String> = plan.writes.keys().cloned().collect();

        assert!(
            write_keys.is_disjoint(&plan.deletes),
            "output plan writes and deletes should be disjoint; transition: {transition:?}"
        );

        implementation_state.apply_plan(plan);

        implementation_state
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
