mod mock;
mod stateless;

use std::num::NonZeroU16;

use proptest::proptest;
use proptest_state_machine::{ReferenceStateMachine, StateMachineTest, prop_state_machine};
use typst_syntax::FileId;

use crate::compiler::Compiler;

use self::mock::{AbstractState, Transition, test_render_config};
use self::stateless::process_stateless;

struct IncrementalMatchesStateless;

impl StateMachineTest for IncrementalMatchesStateless {
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
                // TODO: We thread NonZeroU16 through the ref_state impl
                state.remove(FileId::from_raw(NonZeroU16::new(file_id_raw).unwrap()));
            }
            _ => {
                let compiled = ref_state.compile_file(file_id_raw);
                state._update(
                    FileId::from_raw(NonZeroU16::new(file_id_raw).unwrap()),
                    compiled,
                );
            }
        }

        state
    }

    fn check_invariants(
        state: &Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) {
        // let config = test_render_config();

        // let mut compiler_clone: Compiler = state.clone();
        // let incremental_output = compiler_clone
        //     .process(&config)
        //     .expect("incremental process() failed");

        // let (stateless_output, _stateless_compile_diags, _stateless_process_diags) =
        //     process_stateless(ref_state, &config).expect("stateless process() failed");

        // assert_eq!(
        //     incremental_output.writes, stateless_output,
        //     "incremental output differs from stateless reference"
        // );

        // TODO: Most of that is wrong

        todo!()
    }
}

prop_state_machine! {
    #[test]
    fn incremental_matches_stateless(sequential 1..20 => IncrementalMatchesStateless);
}
