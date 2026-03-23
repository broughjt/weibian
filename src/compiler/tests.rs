use std::borrow::Cow;
use std::collections::HashMap;
use std::num::NonZeroU16;
use std::ops::RangeInclusive;

use proptest::collection::{hash_set, vec};
use proptest::prelude::*;
use proptest::sample::subsequence;

use crate::compiler::{Compile, CompileOutput, Compiler, OutputPlan};
use crate::config::{LINK_TEMPLATE, NODE_TEMPLATE, RenderConfig, TRANSCLUSION_TEMPLATE};
use ecow::EcoVec;
use typst::diag::{SourceDiagnostic, Warned};
use typst::syntax::{FileId, Span};

proptest! {
    #[test]
    fn compile_scratch_equal_compile_incremental_batched(batches in arbitrary_event_batches(&EventConfig::from_environment())) {
        let config = render_config();

        let scratch = {
            let mut compiler = Compiler::default();
            for (id, node) in reduce_events(batches.iter().flatten()) {
                compiler.update(node, file_id(id));
            }
            let mut files = HashMap::new();
            apply(compiler.process(&config).unwrap(), &mut files);
            files
        };

        let incremental = {
            let mut compiler = Compiler::default();
            let mut files = HashMap::new();
            for batch in &batches {
                for event in batch {
                    match event {
                        Event::Update(id, node) => compiler.update(node, file_id(*id)),
                        Event::Remove(id) => compiler.remove(file_id(*id)),
                    }
                }
                apply(compiler.process(&config).unwrap(), &mut files);
            }
            files
        };

        prop_assert_eq!(scratch, incremental);
    }

    #[test]
    fn compile_scratch_equal_compile_incremental(events in arbitrary_events(&EventConfig::from_environment())) {
        let config = render_config();

        let scratch = {
            let mut compiler = Compiler::default();
            for (id, node) in reduce_events(events.iter()) {
                compiler.update(node, file_id(id));
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
                    Event::Update(id, node) => compiler.update(node, file_id(*id)),
                    Event::Remove(id) => compiler.remove(file_id(*id)),
                }
                apply(compiler.process(&config).unwrap(), &mut files);
            }
            files
        };

        prop_assert_eq!(scratch, incremental);
    }

    #[test]
    fn process_is_idempotent(events in arbitrary_events(&EventConfig::from_environment())) {
        let config = render_config();
        let mut compiler = Compiler::default();

        for event in &events {
            match event {
                Event::Update(id, node) => compiler.update(node, file_id(*id)),
                Event::Remove(id) => compiler.remove(file_id(*id)),
            }
        }
        compiler.process(&config).unwrap();

        let second = compiler.process(&config).unwrap();
        prop_assert!(second.writes.is_empty(), "expected no writes on second process call, got {:?}", second.writes.keys().collect::<Vec<_>>());
        prop_assert!(second.deletes.is_empty(), "expected no deletes on second process call, got {:?}", second.deletes);
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

struct EventConfig {
    pool_size: RangeInclusive<usize>,
    sequence_length: RangeInclusive<usize>,
    batch_count: RangeInclusive<usize>,
    max_transcludes: usize,
    max_links: usize,
}

impl EventConfig {
    fn from_environment() -> Self {
        Self {
            pool_size: std::env::var("TEST_POOL_SIZE")
                .ok()
                .as_deref()
                .map(parse_size_range)
                .unwrap_or(1..=8),
            sequence_length: std::env::var("TEST_SEQUENCE_LENGTH")
                .ok()
                .as_deref()
                .map(parse_size_range)
                .unwrap_or(1..=16),
            batch_count: std::env::var("TEST_BATCH_COUNT")
                .ok()
                .as_deref()
                .map(parse_size_range)
                .unwrap_or(1..=4),
            max_transcludes: std::env::var("TEST_MAX_TRANSCLUDES")
                .ok()
                .map(|s| s.parse().expect("TEST_MAX_TRANSCLUDES must be a number"))
                .unwrap_or(3),
            max_links: std::env::var("TEST_MAX_LINKS")
                .ok()
                .map(|s| s.parse().expect("TEST_MAX_LINKS must be a number"))
                .unwrap_or(3),
        }
    }
}

fn arbitrary_mock_node(
    id: NonZeroU16,
    nodes: Cow<'static, [NonZeroU16]>,
    config: &EventConfig,
) -> impl Strategy<Value = MockNode> {
    let max_transcludes = nodes.len().min(config.max_transcludes);
    let max_links = nodes.len().min(config.max_links);
    let transcludes = subsequence(nodes.clone(), 0..=max_transcludes);
    let links = subsequence(nodes, 0..=max_links);

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

fn arbitrary_remove(nodes: Cow<'static, [NonZeroU16]>) -> impl Strategy<Value = Event> {
    proptest::sample::select(nodes).prop_map(Event::Remove)
}

fn arbitrary_update(
    nodes: Cow<'static, [NonZeroU16]>,
    config: &EventConfig,
) -> impl Strategy<Value = Event> {
    proptest::sample::select(nodes.clone()).prop_flat_map(move |id| {
        arbitrary_mock_node(id, nodes.clone(), config).prop_map(move |node| Event::Update(id, node))
    })
}

fn arbitrary_event(
    nodes: Cow<'static, [NonZeroU16]>,
    config: &EventConfig,
) -> impl Strategy<Value = Event> {
    prop_oneof![
        arbitrary_update(nodes.clone(), config),
        arbitrary_remove(nodes),
    ]
}

fn arbitrary_event_batches(config: &EventConfig) -> impl Strategy<Value = Vec<Vec<Event>>> {
    hash_set(any::<NonZeroU16>(), config.pool_size.clone()).prop_flat_map(move |ids| {
        let nodes: Cow<'static, [NonZeroU16]> = Cow::Owned(ids.into_iter().collect());
        let length = config.sequence_length.clone();
        let batch_count = config.batch_count.clone();

        (vec(arbitrary_event(nodes, config), length), batch_count).prop_flat_map(|(events, k)| {
            let n = events.len();
            vec(0..=n, k.saturating_sub(1)).prop_map(move |mut points| {
                points.sort_unstable();
                let mut batches = Vec::with_capacity(k);
                let mut prev = 0;
                for point in points {
                    batches.push(events[prev..point].to_vec());
                    prev = point;
                }
                batches.push(events[prev..].to_vec());
                batches
            })
        })
    })
}

fn arbitrary_events(config: &EventConfig) -> impl Strategy<Value = Vec<Event>> {
    hash_set(any::<NonZeroU16>(), config.pool_size.clone()).prop_flat_map(move |ids| {
        let nodes: Cow<'static, [NonZeroU16]> = Cow::Owned(ids.into_iter().collect());
        let length = config.sequence_length.clone();

        vec(arbitrary_event(nodes, config), length)
    })
}

fn parse_size_range(s: &str) -> RangeInclusive<usize> {
    if let Some((lo, hi)) = s.split_once("..") {
        let lo = lo.parse().expect("invalid lower bound in size range");
        let hi = hi.parse().expect("invalid upper bound in size range");
        lo..=hi
    } else {
        let n = s
            .parse()
            .expect("TEST_* size value must be a number or range");
        n..=n
    }
}

fn reduce_events<'a>(events: impl Iterator<Item = &'a Event>) -> HashMap<NonZeroU16, &'a MockNode> {
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

fn render_config() -> RenderConfig {
    let node_template = "{{ node.body | safe }}".to_string();
    let transclusion_template = concat!(
        "{%- if transclusion.resolved -%}",
        "{{ transclusion.body | safe }}",
        "{%- else -%}",
        r#"<wb-missing identifier="{{ transclusion.identifier }}"></wb-missing>"#,
        "{%- endif -%}",
    )
    .to_string();
    let link_template = concat!(
        "{%- if link.resolved -%}",
        r#"<span class="link local"><a href="{{ link.href }}">"#,
        "{%- if link.content %}{{ link.content | safe }}{%- else %}{{ link.title | safe }}{%- endif %}",
        "</a></span>",
        "{%- else -%}",
        r#"<span class="link local"><a href="{{ link.href }}">{{ link.content | safe }}</a></span>"#,
        "{%- endif -%}",
    )
    .to_string();

    let mut environment = minijinja::Environment::new();
    environment
        .add_template_owned(NODE_TEMPLATE, node_template)
        .unwrap();
    environment
        .add_template_owned(TRANSCLUSION_TEMPLATE, transclusion_template)
        .unwrap();
    environment
        .add_template_owned(LINK_TEMPLATE, link_template)
        .unwrap();
    RenderConfig {
        root_directory: "/".to_string(),
        trailing_slash: false,
        index_node: "index".to_string(),
        domain: String::new(),
        environment,
    }
}

fn format_id(id: NonZeroU16) -> String {
    format!("n{id}")
}
