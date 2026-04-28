mod model;
mod process_stateless;
mod reference_compiler;
mod render;

use std::{collections::HashMap, num::NonZeroU16};

use proptest_state_machine::{ReferenceStateMachine, StateMachineTest, prop_state_machine};

use crate::compiler::{
    Compiler, OutputPlan,
    tests::{
        model::MockNode,
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
        apply_plan(&mut state.filesystem, plan);

        state
    }

    fn check_invariants(
        state: &Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) {
        // Files with compile errors are dropped by the incremental compiler
        // (see Compiler::_update), so exclude them from the stateless input
        // to keep both sides agreeing on the node set.
        let input: HashMap<NonZeroU16, HashMap<String, MockNode>> = ref_state
            .files
            .iter()
            .filter(|(_, file)| file.errors.is_empty())
            .map(|(file_id, file)| {
                let nodes: HashMap<String, MockNode> = file
                    .nodes
                    .iter()
                    .map(|node_id| {
                        let mock = &ref_state.nodes[node_id];
                        (mock.identifier.0.to_string(), mock.clone())
                    })
                    .collect();
                (file_id.into_raw(), nodes)
            })
            .collect();

        let (expected_output, expected_compile_diagnostics, expected_process_diagnostics) =
            process_stateless(&input).expect("stateless reference must succeed");

        assert_eq!(state.filesystem, expected_output);
        assert_eq!(
            state.compiler.compile_diagnostics(),
            &expected_compile_diagnostics
        );
        assert_eq!(
            state.compiler.process_diagnostics(),
            &expected_process_diagnostics
        );
    }
}

fn apply_plan(filesystem: &mut HashMap<String, RenderNode>, plan: OutputPlan<RenderNode>) {
    // TODO: Could test that inserts and removes are always disjoint in both the reference compiler and the incremental compiler
    for (name, node) in plan.writes {
        filesystem.insert(name, node);
    }
    for name in plan.deletes {
        filesystem.remove(&name);
    }
}

prop_state_machine! {
    #[test]
    fn incremental_matches_stateless(sequential 1..20 => IncrementalMatchesStateless);
}
