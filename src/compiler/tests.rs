mod mock;
mod stateless;

use proptest_state_machine::{ReferenceStateMachine, StateMachineTest, prop_state_machine};

use crate::compiler::Compiler;

use self::mock::{AbstractState, Transition, test_render_config};
use self::stateless::process_stateless;

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

        let mut compiler_clone: Compiler = state.clone();
        let incremental_output = compiler_clone
            .process(&config)
            .expect("incremental process() failed");

        let (stateless_output, _stateless_compile_diags, _stateless_process_diags) =
            process_stateless(ref_state, &config).expect("stateless process() failed");

        assert_eq!(
            incremental_output.writes, stateless_output,
            "incremental output differs from stateless reference"
        );
    }
}

prop_state_machine! {
    #[test]
    fn incremental_matches_stateless(sequential 1..20 => CompilerStateMachineTest);
}
