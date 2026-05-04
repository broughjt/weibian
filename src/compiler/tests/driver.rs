use std::sync::{
    Arc,
    atomic::{self, AtomicUsize},
};

use proptest_state_machine::{ReferenceStateMachine, StateMachineTest};

// /// A cheap rip-off of [`StateMachineTest`] that is new and improved.
// pub trait StateMachineTestDeluxe {
//     type Implementation;
//     type Specification: ReferenceStateMachine;

//     fn initial_state(
//         specification_state: &<Self::Specification as ReferenceStateMachine>::State,
//     ) -> Self::Implementation;

//     fn apply(
//         implementation_state: Self::Implementation,
//         specification_state_before: &<Self::Specification as ReferenceStateMachine>::State,
//         specification_state_after: &<Self::Specification as ReferenceStateMachine>::State,
//         transition: <Self::Specification as ReferenceStateMachine>::Transition,
//     ) -> Self::Implementation;

//     fn check_invariants(
//         implementation_state: &Self::Implementation,
//         specification: &<Self::Specification as ReferenceStateMachine>::State,
//     ) {
//         let _ = (implementation_state, specification);
//     }
// }

pub fn run_sequential<T: StateMachineTest>(
    mut specification_state: <T::Reference as ReferenceStateMachine>::State,
    transitions: Vec<<T::Reference as ReferenceStateMachine>::Transition>,
    mut seen_counter: Option<Arc<AtomicUsize>>,
) {
    let mut implementation_state = T::init_test(&specification_state);

    T::check_invariants(&implementation_state, &specification_state);

    for transition in transitions.into_iter() {
        if let Some(seen_counter) = seen_counter.as_mut() {
            seen_counter.fetch_add(1, atomic::Ordering::SeqCst);
        }

        specification_state =
            <T::Reference as ReferenceStateMachine>::apply(specification_state, &transition);
        implementation_state = T::apply(implementation_state, &specification_state, transition);

        T::check_invariants(&implementation_state, &specification_state);
    }

    // T::teardown(state);
}

// pub fn run_batched<T: StateMachineTestDeluxe>(
//     mut specification_state: <T::Specification as ReferenceStateMachine>::State,
//     transitions: Vec<<T::Specification as ReferenceStateMachine>::Transition>,
//     mut seen_counter: Option<Arc<AtomicUsize>>,
//     batch_sizes: Vec<usize>,
// ) {
//     let batches = {
//         let mut transitions = transitions.into_iter();
//         batch_sizes
//             .into_iter()
//             .map(|size| transitions.by_ref().take(size).collect::<Vec<_>>())
//             .take_while(|batch| !batch.is_empty())
//             .collect::<Vec<Vec<<T::Specification as ReferenceStateMachine>::Transition>>>()
//     };

//     let mut implementation_state = T::initial_state(&specification_state);

//     T::check_invariants(&implementation_state, &specification_state);

//     for batch in batches {
//         for transition in batch {
//             if let Some(counter) = seen_counter.as_mut() {
//                 counter.fetch_add(1, atomic::Ordering::SeqCst);
//             }
//             let specification_state_next = <T::Specification as ReferenceStateMachine>::apply(
//                 specification_state.clone(),
//                 &transition,
//             );
//             implementation_state = T::apply(
//                 implementation_state,
//                 &specification_state_next,
//                 &specification_state_next,
//                 transition,
//             );
//             specification_state = specification_state_next;
//         }
//     }
// }
