use std::borrow::Cow;
use std::collections::HashMap;
use std::num::NonZeroU16;
use std::ops::RangeInclusive;

use proptest::prelude::*;
use proptest::sample::subsequence;

use crate::compiler::{Compile, CompileOutput, Compiler, OutputPlan};
use crate::config::{LINK_TEMPLATE, NODE_TEMPLATE, RenderConfig, TRANSCLUSION_TEMPLATE};
use ecow::EcoVec;
use typst::diag::{SourceDiagnostic, Warned};
use typst::syntax::{FileId, Span};

proptest! {
    #[test]
    fn compile_scratch_equal_compile_incremental(events in arbitrary_events(EventConfig::default())) {
        let config = test_render_config();

        let scratch = {
            let mut compiler = Compiler::default();
            for (id, node) in reduce_events(&events) {
                compiler.update(node.clone(), file_id(id));
            }
            let mut files = HashMap::new();
            apply(compiler.process(&config).unwrap(), &mut files);
            files
        };

        let incremental = {
            let mut compiler = Compiler::default();
            let mut files = HashMap::new();
            for event in &events {
                match event {
                    Event::Update(id, node) => compiler.update(node.clone(), file_id(*id)),
                    Event::Remove(id) => compiler.remove(file_id(*id)),
                }
                apply(compiler.process(&config).unwrap(), &mut files);
            }
            files
        };

        prop_assert_eq!(scratch, incremental);
    }
}

/// A single mock file: one primary node with edges to other nodes by ID.
#[derive(Debug, Clone)]
struct MockNode {
    id: NonZeroU16,
    title: String,
    body: String,
    transcludes: Vec<NonZeroU16>,
    links: Vec<NonZeroU16>,
}

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

#[derive(Debug, Clone)]
enum Event {
    Update(NonZeroU16, MockNode),
    Remove(NonZeroU16),
}

#[derive(Clone)]
struct EventConfig {
    pool_size: RangeInclusive<usize>,
    sequence_length: RangeInclusive<usize>,
    max_transcludes: usize,
    max_links: usize,
}

impl Default for EventConfig {
    fn default() -> Self {
        Self {
            pool_size: 1..=8,
            sequence_length: 1..=16,
            max_transcludes: 3,
            max_links: 3,
        }
    }
}

fn arbitrary_mock_node(
    id: NonZeroU16,
    pool: Cow<'static, [NonZeroU16]>,
    config: EventConfig,
) -> impl Strategy<Value = MockNode> {
    let max_transcludes = pool.len().min(config.max_transcludes);
    let max_links = pool.len().min(config.max_links);
    let transcludes = subsequence(pool.clone(), 0..=max_transcludes);
    let links = subsequence(pool, 0..=max_links);

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
            Event::Update(id, node) => {
                result.insert(*id, node);
            }
            Event::Remove(id) => {
                result.remove(id);
            }
        }
    }
    result
}

fn apply(plan: OutputPlan, fs: &mut HashMap<String, String>) {
    for (k, v) in plan.writes {
        fs.insert(k, v);
    }
    for k in plan.deletes {
        fs.remove(&k);
    }
}

fn file_id(id: NonZeroU16) -> FileId {
    FileId::from_raw(id)
}

fn test_render_config() -> RenderConfig {
    let mut environment = minijinja::Environment::new();
    environment
        .add_template_owned(NODE_TEMPLATE, String::new())
        .unwrap();
    environment
        .add_template_owned(TRANSCLUSION_TEMPLATE, String::new())
        .unwrap();
    environment
        .add_template_owned(LINK_TEMPLATE, String::new())
        .unwrap();
    RenderConfig {
        root_directory: "/".to_string(),
        trailing_slash: false,
        index_node: "index".to_string(),
        domain: String::new(),
        environment,
    }
}

fn arbitrary_remove(pool: Cow<'static, [NonZeroU16]>) -> impl Strategy<Value = Event> {
    proptest::sample::select(pool).prop_map(Event::Remove)
}

fn arbitrary_update(
    pool: Cow<'static, [NonZeroU16]>,
    config: EventConfig,
) -> impl Strategy<Value = Event> {
    proptest::sample::select(pool.clone()).prop_flat_map(move |id| {
        arbitrary_mock_node(id, pool.clone(), config.clone())
            .prop_map(move |node| Event::Update(id, node))
    })
}

fn arbitrary_event(
    pool: Cow<'static, [NonZeroU16]>,
    config: EventConfig,
) -> impl Strategy<Value = Event> {
    prop_oneof![
        arbitrary_update(pool.clone(), config),
        arbitrary_remove(pool),
    ]
}

fn arbitrary_events(config: EventConfig) -> impl Strategy<Value = Vec<Event>> {
    proptest::collection::hash_set(any::<NonZeroU16>(), config.pool_size.clone()).prop_flat_map(
        move |ids| {
            let pool: Cow<'static, [NonZeroU16]> = Cow::Owned(ids.into_iter().collect());
            let sequence_length = config.sequence_length.clone();
            proptest::collection::vec(arbitrary_event(pool, config.clone()), sequence_length)
        },
    )
}

fn format_id(id: NonZeroU16) -> String {
    format!("n{id}")
}
