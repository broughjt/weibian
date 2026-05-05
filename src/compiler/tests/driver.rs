use std::sync::{
    Arc,
    atomic::{self, AtomicUsize},
};

use proptest::{
    collection::SizeRange,
    prelude::{Just, Strategy},
};
use proptest_state_machine::{ReferenceStateMachine, StateMachineTest};

pub fn run_sequential<T: StateMachineTest>(
    mut specification_state: <T::Reference as ReferenceStateMachine>::State,
    transitions: Vec<<T::Reference as ReferenceStateMachine>::Transition>,
    mut seen_counter: Option<Arc<AtomicUsize>>,
) {
    let mut implementation_state = T::init_test(&specification_state);

    T::check_invariants(&implementation_state, &specification_state);

    for transition in transitions {
        if let Some(seen_counter) = seen_counter.as_mut() {
            seen_counter.fetch_add(1, atomic::Ordering::SeqCst);
        }

        specification_state =
            <T::Reference as ReferenceStateMachine>::apply(specification_state, &transition);
        implementation_state = T::apply(implementation_state, &specification_state, transition);

        T::check_invariants(&implementation_state, &specification_state);
    }

    T::teardown(implementation_state, specification_state);
}

pub trait StateMachineTestBatched: StateMachineTest {
    fn apply_batch(
        implementation_state: Self::SystemUnderTest,
        specification_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) -> Self::SystemUnderTest;
}

/// Output of `ReferenceStateMachine::sequential_strategy`: initial state, the
/// generated transition sequence, and proptest's per-step seen-counter (used
/// during shrinking to mark transitions that were actually applied).
pub type StateMachineInput<T> = (
    <<T as StateMachineTest>::Reference as ReferenceStateMachine>::State,
    Vec<<<T as StateMachineTest>::Reference as ReferenceStateMachine>::Transition>,
    Option<Arc<AtomicUsize>>,
);

/// Strategy producing batched test inputs: the underlying state-machine
/// strategy paired with a batch-size schedule of `batch_sizes_strategy(n)`,
/// where `n` is the realized transition count.
pub fn batched_strategy<T, S>(
    transitions: impl Into<SizeRange>,
    batch_sizes_strategy: impl Fn(usize) -> S,
) -> impl Strategy<Value = (StateMachineInput<T>, Vec<usize>)>
where
    T: StateMachineTestBatched,
    S: Strategy<Value = Vec<usize>>,
{
    <T::Reference as ReferenceStateMachine>::sequential_strategy(transitions).prop_flat_map(
        move |u| {
            let n = u.1.len();
            (Just(u), batch_sizes_strategy(n))
        },
    )
}

pub fn run_batched<T: StateMachineTestBatched>(
    mut specification_state: <T::Reference as ReferenceStateMachine>::State,
    transitions: Vec<<T::Reference as ReferenceStateMachine>::Transition>,
    mut seen_counter: Option<Arc<AtomicUsize>>,
    batch_sizes: Vec<usize>,
) {
    let batches = {
        let mut transitions = transitions.into_iter().peekable();
        let batches = batch_sizes
            .into_iter()
            .map_while(|size| {
                assert!(size > 0, "batch sizes must be positive");

                transitions
                    .peek()
                    .is_some()
                    .then(|| transitions.by_ref().take(size).collect::<Vec<_>>())
            })
            .collect::<Vec<Vec<<T::Reference as ReferenceStateMachine>::Transition>>>();

        assert!(
            transitions.peek().is_none(),
            "batch sizes did not cover all transitions"
        );

        batches
    };

    let mut implementation_state = T::init_test(&specification_state);

    T::check_invariants(&implementation_state, &specification_state);

    for batch in batches {
        for transition in batch {
            if let Some(counter) = seen_counter.as_mut() {
                counter.fetch_add(1, atomic::Ordering::SeqCst);
            }
            specification_state =
                <T::Reference as ReferenceStateMachine>::apply(specification_state, &transition);
            implementation_state = T::apply(implementation_state, &specification_state, transition);
        }
        implementation_state = T::apply_batch(implementation_state, &specification_state);

        T::check_invariants(&implementation_state, &specification_state);
    }

    T::teardown(implementation_state, specification_state);
}

pub trait StateMachineTestCompared {
    type Implementation;
    type Specification: ReferenceStateMachine;

    fn new(
        specification_state: &<Self::Specification as ReferenceStateMachine>::State,
    ) -> Self::Implementation;

    fn apply(
        implementation_state: Self::Implementation,
        specification_state_before: &<Self::Specification as ReferenceStateMachine>::State,
        specification_state_after: &<Self::Specification as ReferenceStateMachine>::State,
        transition: <Self::Specification as ReferenceStateMachine>::Transition,
    ) -> Self::Implementation;

    fn check(
        implementation_state: &Self::Implementation,
        specification_state: &<Self::Specification as ReferenceStateMachine>::State,
    ) {
        let _ = (implementation_state, specification_state);
    }
}

pub fn run_compared<T: StateMachineTestCompared>(
    mut specification_state: <T::Specification as ReferenceStateMachine>::State,
    transitions: Vec<<T::Specification as ReferenceStateMachine>::Transition>,
    mut seen_counter: Option<Arc<AtomicUsize>>,
) {
    let mut implementation_state = T::new(&specification_state);

    T::check(&implementation_state, &specification_state);

    for transition in transitions {
        if let Some(seen_counter) = seen_counter.as_mut() {
            seen_counter.fetch_add(1, atomic::Ordering::SeqCst);
        }

        let specification_state_next = <T::Specification as ReferenceStateMachine>::apply(
            specification_state.clone(),
            &transition,
        );
        implementation_state = T::apply(
            implementation_state,
            &specification_state,
            &specification_state_next,
            transition,
        );
        specification_state = specification_state_next;

        T::check(&implementation_state, &specification_state);
    }
}
