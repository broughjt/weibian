use proptest_state_machine::StateMachineTest;

mod mock;
mod stateless;

struct IncrementalMatchesStateless;

impl StateMachineTest for IncrementalMatchesStateless {
    type SystemUnderTest;
    type Reference;

    fn init_test(
        ref_state: &<Self::Reference as proptest_state_machine::ReferenceStateMachine>::State,
    ) -> Self::SystemUnderTest {
        todo!()
    }

    fn apply(
        state: Self::SystemUnderTest,
        ref_state: &<Self::Reference as proptest_state_machine::ReferenceStateMachine>::State,
        transition: <Self::Reference as proptest_state_machine::ReferenceStateMachine>::Transition,
    ) -> Self::SystemUnderTest {
        todo!()
    }
}

prop_state_machine! {
    #[test]
    fn incremental_matches_stateless(sequential 1..20 => IncrementalMatchesStateless);
}
