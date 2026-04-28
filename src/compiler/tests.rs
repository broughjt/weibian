mod model;
mod process_stateless;
mod reference_compiler;
mod render;

use std::{collections::HashMap, num::NonZeroU16};

use ecow::{EcoVec, eco_format};
use proptest_state_machine::{ReferenceStateMachine, StateMachineTest, prop_state_machine};
use typst::diag::{SourceDiagnostic, Warned};
use typst_syntax::{FileId, Span};

use crate::compiler::{
    Compiler, OutputPlan,
    extract::NodeOutput,
    tests::{
        model::MockNode,
        process_stateless::process_stateless,
        reference_compiler::{FileState, MockNodeId, ReferenceCompiler, Transition},
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
            let file = ref_state
                .files
                .get(&file_id)
                .expect("bug: ref_state must contain affected file_id after non-remove transition");
            let warned = compile_payload(file, &ref_state.nodes);
            state.compiler._update(file_id, warned);
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

        let (expected_output, _expected_compile_diagnostics, expected_process_diagnostics) =
            process_stateless(&input).expect("stateless reference must succeed");

        assert_eq!(state.filesystem, expected_output);
        assert_eq!(
            state.compiler.process_diagnostics(),
            &expected_process_diagnostics
        );
        // TODO: Check compile_diagnostics
    }
}

fn compile_payload(
    file: &FileState,
    all_nodes: &HashMap<MockNodeId, MockNode>,
) -> Warned<Result<HashMap<String, NodeOutput>, EcoVec<SourceDiagnostic>>> {
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
                let mock = &all_nodes[node_id];
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

fn apply_plan(filesystem: &mut HashMap<String, RenderNode>, plan: OutputPlan<RenderNode>) {
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
