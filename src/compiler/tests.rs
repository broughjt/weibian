mod model;
mod process_stateless;
mod reference_compiler;
mod render;

use std::collections::HashMap;

use proptest_state_machine::{ReferenceStateMachine, StateMachineTest, prop_state_machine};

use crate::compiler::{
    Compiler,
    tests::{reference_compiler::ReferenceCompiler, render::RenderNode},
};

struct IncrementalMatchesStateless;

struct IncrementalCompiler {
    compiler: Compiler,
    filesystem: HashMap<String, RenderNode>,
}

impl StateMachineTest for IncrementalMatchesStateless {
    type SystemUnderTest = IncrementalCompiler;
    type Reference = ReferenceCompiler;

    fn init_test(
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) -> Self::SystemUnderTest {
        todo!()
    }

    fn apply(
        state: Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
        transition: <Self::Reference as ReferenceStateMachine>::Transition,
    ) -> Self::SystemUnderTest {
        todo!()
    }

    fn check_invariants(
        state: &Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) {
        todo!()
    }
}

prop_state_machine! {
    #[test]
    fn incremental_matches_stateless(sequential 1..20 => IncrementalMatchesStateless);
}
