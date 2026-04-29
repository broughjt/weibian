mod model;
mod process_stateless;
mod reference_compiler;
mod render;

use std::collections::HashMap;

use ecow::EcoVec;
use proptest_state_machine::{ReferenceStateMachine, StateMachineTest, prop_state_machine};
use typst::diag::{Severity, SourceDiagnostic};

use crate::compiler::{
    CompileDiagnostics, Compiler, ProcessDiagnostics,
    tests::{
        process_stateless::process_stateless,
        reference_compiler::{ReferenceCompiler, Transition},
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
        // TODO: Could test that inserts and removes are always disjoint in both the reference compiler and the incremental compiler
        for (name, node) in plan.writes {
            state.filesystem.insert(name, node);
        }
        for name in plan.deletes {
            state.filesystem.remove(&name);
        }

        state
    }

    fn check_invariants(
        state: &Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) {
        let (expected_output, expected_compile_diagnostics, expected_process_diagnostics) =
            process_stateless(ref_state).expect("stateless reference must succeed");

        assert_eq!(state.filesystem, expected_output);
        assert_eq!(
            normalize_compile_diagnostics(state.compiler.compile_diagnostics()),
            normalize_compile_diagnostics(&expected_compile_diagnostics),
        );
        assert_eq!(
            normalize_process_diagnostics(state.compiler.process_diagnostics()),
            normalize_process_diagnostics(&expected_process_diagnostics),
        );
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
    fn incremental_matches_stateless(sequential 1..20 => IncrementalMatchesStateless);
}
