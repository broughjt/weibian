use std::borrow::Cow;
use std::collections::HashMap;
use std::num::NonZeroU16;

use proptest::prelude::*;
use proptest::sample::subsequence;

use crate::compiler::{Compile, CompileOutput};
use ecow::EcoVec;
use typst::diag::{SourceDiagnostic, Warned};
use typst::syntax::{FileId, Span};

/// A single mock file: one primary node with edges to other nodes by ID.
#[derive(Debug, Clone)]
struct MockNode {
    id: NonZeroU16,
    title: String,
    body: String,
    transcludes: Vec<NonZeroU16>,
    links: Vec<NonZeroU16>,
}

#[derive(Debug, Clone)]
enum Event {
    Update(NonZeroU16, MockNode),
    Remove(NonZeroU16),
}

// TODO: Prolly impelement instead for (NonZeroU16, MockNode) pairs, not sure yet
impl Compile for MockNode {
    fn compile(&self, _id: FileId) -> Warned<Result<CompileOutput, EcoVec<SourceDiagnostic>>> {
        let node_id = format_id(self.id);

        let mut html = format!(r#"<wb-node identifier="{node_id}">"#);
        html.push_str(&format!("<wb-title>{}</wb-title>", self.title));
        html.push_str(&format!("<p>{}</p>", self.body));

        for (counter, &target) in self.transcludes.iter().enumerate() {
            let target_id = format_id(target);
            html.push_str(&format!(
                r#"<wb-transclude identifier="{target_id}" counter="{counter}"></wb-transclude>"#
            ));
        }

        for (counter, &target) in self.links.iter().enumerate() {
            let target_id = format_id(target);
            html.push_str(&format!(
                r#"<a href="wb:{target_id}" data-counter="{counter}">link</a>"#
            ));
        }

        html.push_str("</wb-node>");

        Warned {
            output: Ok(CompileOutput {
                html,
                spans: HashMap::from([(node_id, Span::detached())]),
                // TODO: test metadata propagation
                metadata: HashMap::new(),
                // Counters in the HTML elements above are consumed by extract; these
                // maps are intentionally empty (no per-edge metadata on mock nodes).
                transclusion_metadata: HashMap::new(),
                link_metadata: HashMap::new(),
                // TODO: test error cases
                errors: EcoVec::new(),
            }),
            warnings: EcoVec::new(),
        }
    }
}

fn arbitrary_mock_node(
    id: NonZeroU16,
    nodes: Cow<'static, [NonZeroU16]>,
) -> impl Strategy<Value = MockNode> {
    let transcludes = subsequence(nodes.clone(), 0..=3);
    let links = subsequence(nodes, 0..=3);

    ("[a-z]+", "[a-z]*", transcludes, links).prop_map(move |(title, body, transcludes, links)| {
        MockNode {
            id,
            title,
            body,
            transcludes,
            links,
        }
    })
}

fn reduce_events(events: &[Event]) -> HashMap<NonZeroU16, &MockNode> {
    let mut result = HashMap::new();
    for event in events {
        match event {
            Event::Update(id, node) => { result.insert(*id, node); }
            Event::Remove(id) => { result.remove(id); }
        }
    }
    result
}

proptest! {
    #[test]
    fn compile_scratch_equal_compile_incremental(_events in arbitrary_events()) {
        todo!("implement scratch vs incremental comparison")
    }
}

fn arbitrary_remove(pool: Cow<'static, [NonZeroU16]>) -> impl Strategy<Value = Event> {
    proptest::sample::select(pool).prop_map(Event::Remove)
}

fn arbitrary_update(pool: Cow<'static, [NonZeroU16]>) -> impl Strategy<Value = Event> {
    proptest::sample::select(pool.clone())
        .prop_flat_map(move |id| {
            arbitrary_mock_node(id, pool.clone()).prop_map(move |node| Event::Update(id, node))
        })
}

fn arbitrary_event(pool: Cow<'static, [NonZeroU16]>) -> impl Strategy<Value = Event> {
    prop_oneof![
        arbitrary_update(pool.clone()),
        arbitrary_remove(pool),
    ]
}

fn arbitrary_events() -> impl Strategy<Value = Vec<Event>> {
    proptest::collection::hash_set(any::<NonZeroU16>(), 1..=8)
        .prop_flat_map(|ids| {
            let pool: Cow<'static, [NonZeroU16]> = Cow::Owned(ids.into_iter().collect());
            proptest::collection::vec(arbitrary_event(pool), 1..=16)
        })
}

fn format_id(id: NonZeroU16) -> String {
    format!("n{id}")
}
