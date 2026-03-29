use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::num::NonZeroU16;
use std::ops::RangeInclusive;

use proptest::collection::{hash_set, vec};
use proptest::prelude::*;
use proptest::sample::subsequence;

use crate::compiler::{Compile, CompileOutput, Compiler, OutputPlan};
use crate::config::{
    BACKMATTER_TEMPLATE, LINK_TEMPLATE, NODE_TEMPLATE, RenderConfig, TRANSCLUSION_TEMPLATE,
};
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
    fn output_plan_writes_and_deletes_are_disjoint(events in arbitrary_events(&EventConfig::from_environment())) {
        let config = render_config();
        let mut compiler = Compiler::default();

        for event in &events {
            match event {
                Event::Update(id, node) => compiler.update(node, file_id(*id)),
                Event::Remove(id) => compiler.remove(file_id(*id)),
            }
        }
        let plan = compiler.process(&config).unwrap();
        let writes: HashSet<&str> = plan.writes.keys().map(String::as_str).collect();
        let deletes: HashSet<&str> = plan.deletes.iter().map(String::as_str).collect();

        prop_assert!(writes.is_disjoint(&deletes));
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

    /// A title-only change to node B must cause any node whose backmatter
    /// references B (via contexts, backlinks, or outlinks) to appear in the
    /// next OutputPlan's writes.
    #[test]
    fn metadata_change_triggers_backmatter_rerender(
        events in arbitrary_events(&EventConfig::from_environment()),
        changed_id in any::<NonZeroU16>(),
    ) {
        let config = render_config();
        let mut compiler = Compiler::default();

        for event in &events {
            match event {
                Event::Update(id, node) => compiler.update(node, file_id(*id)),
                Event::Remove(id) => compiler.remove(file_id(*id)),
            }
        }
        let first = compiler.process(&config).unwrap();
        let mut files: HashMap<String, String> = HashMap::new();
        apply(first, &mut files);

        // Emit a title-only update for changed_id (no edge changes).
        let node_id_str = format_id(changed_id);
        let changed_file = file_id(changed_id);
        if !compiler.has_node(changed_id) {
            // Node doesn't exist yet; insert a fresh one with no edges.
            let node = MockNode { id: changed_id, title: "before".into(), body: String::new(), transcludes: vec![], links: vec![] };
            compiler.update(&node, changed_file);
            compiler.process(&config).unwrap();
        }

        // Update with a new title, keeping edges identical.
        let new_node = MockNode { id: changed_id, title: "after".into(), body: String::new(), transcludes: vec![], links: vec![] };
        compiler.update(&new_node, changed_file);
        let second = compiler.process(&config).unwrap();

        // changed_id itself must be rewritten (body dirty).
        if second.writes.contains_key(&node_id_str) || files.contains_key(&node_id_str) {
            prop_assert!(second.writes.contains_key(&node_id_str),
                "node {node_id_str} was not rewritten after title change");
        }

        // Every node whose rendered output previously contained changed_id in
        // its backmatter must be rewritten.
        for (name, old_html) in &files {
            let references_changed =
                old_html.contains(&format!("<ctx>{node_id_str}</ctx>"))
                || old_html.contains(&format!("<bl>{node_id_str}</bl>"))
                || old_html.contains(&format!("<ol>{node_id_str}</ol>"));
            if references_changed {
                prop_assert!(
                    second.writes.contains_key(name.as_str()),
                    "node {name} references {node_id_str} in backmatter but was not rewritten"
                );
            }
        }
    }

    /// After each event, the incremental compiler's filesystem state must
    /// exactly match `process_stateless` applied to the current node set.
    ///
    /// This catches mid-sequence bugs (e.g. skipped backmatter rerenders) that
    /// the scratch-vs-incremental tests miss because those tests only compare
    /// the final state after collapsing all events via `reduce_events`.
    #[test]
    fn compile_incremental_matches_reference(
        events in arbitrary_events(&EventConfig::from_environment())
    ) {
        let config = render_config();
        let mut incremental = Compiler::default();
        let mut inc_fs: HashMap<String, String> = HashMap::new();
        let mut current_nodes: HashMap<FileId, MockNode> = HashMap::new();

        for event in &events {
            match event {
                Event::Update(id, node) => {
                    incremental.update(node, file_id(*id));
                    current_nodes.insert(file_id(*id), node.clone());
                }
                Event::Remove(id) => {
                    incremental.remove(file_id(*id));
                    current_nodes.remove(&file_id(*id));
                }
            }
            apply(incremental.process(&config).unwrap(), &mut inc_fs);
            let ref_fs = process_stateless(
                &current_nodes.values().cloned().collect::<Vec<_>>(),
                &config,
            )
            .unwrap();
            prop_assert_eq!(&inc_fs, &ref_fs);
        }
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
                .unwrap_or(1..=12),
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
    let node_template = "{{ node.body | safe }}{{ node.backmatter | safe }}".to_string();
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

    // Backmatter template: renders contexts, backlinks, outlinks as plain ID
    // lists so the property tests can observe backmatter content changes.
    let backmatter_template = concat!(
        "{%- for n in backmatter.contexts -%}<ctx>{{ n.id }}</ctx>{%- endfor -%}",
        "{%- for n in backmatter.backlinks -%}<bl>{{ n.id }}</bl>{%- endfor -%}",
        "{%- for n in backmatter.outlinks -%}<ol>{{ n.id }}</ol>{%- endfor -%}",
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
    environment
        .add_template_owned(BACKMATTER_TEMPLATE, backmatter_template)
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

/// Stateless reference implementation: compiles `mock_nodes` from scratch and
/// returns the complete rendered filesystem as a map of node name → HTML.
///
/// Has no notion of previous state — no dirty/removed/metadata_dirty sets.
/// Every call recomputes everything. Used as a test oracle against the
/// incremental `Compiler`.
fn process_stateless(
    mock_nodes: &[MockNode],
    config: &RenderConfig,
) -> anyhow::Result<HashMap<String, String>> {
    use std::collections::BTreeSet;

    use petgraph::algo::tarjan_scc;
    use petgraph::graphmap::DiGraphMap;

    use super::{
        NodeId, NodeInterner, backmatter_cache, extract, render_backmatter, render_body,
        render_node,
    };

    let mut interner = NodeInterner::default();
    let mut links: DiGraphMap<NodeId, ()> = DiGraphMap::new();
    let mut transclusions: DiGraphMap<NodeId, ()> = DiGraphMap::new();
    let mut all_nodes: HashMap<NodeId, super::NodeEntry> = HashMap::new();

    for mock_node in mock_nodes {
        let typst::diag::Warned { output: result, .. } = mock_node.compile(file_id(mock_node.id));
        if let Ok(output) = result
            && let Ok(extracted) = extract(output, &mut interner, |_| false)
        {
            for (node_id, (entry, trans, lnks)) in extracted {
                for &t in &trans {
                    transclusions.add_edge(node_id, t, ());
                }
                for &l in &lnks {
                    links.add_edge(node_id, l, ());
                }
                all_nodes.insert(node_id, entry);
            }
        }
    }

    let mut unrenderable: HashSet<NodeId> = HashSet::new();
    let mut outlinks_accumulator: HashMap<NodeId, BTreeSet<NodeId>> = HashMap::new();
    let mut render_order: Vec<NodeId> = Vec::new();

    let sccs = tarjan_scc(&transclusions);

    for scc in &sccs {
        let id = scc[0];
        let is_cyclic = scc.len() > 1 || transclusions.contains_edge(id, id);

        if is_cyclic {
            unrenderable.extend(scc.iter().copied());
        } else if transclusions.neighbors(id).any(|t| unrenderable.contains(&t)) {
            unrenderable.insert(id);
        } else if let Some(entry) = all_nodes.get_mut(&id) {
            let new_cache = backmatter_cache(id, &links, &transclusions, &outlinks_accumulator);
            outlinks_accumulator.insert(id, new_cache.outlinks.clone());
            entry.backmatter_cache = Some(new_cache);
            render_order.push(id);
        }
    }

    // Nodes that appear in no transclusion edge (neither source nor target) are
    // not visited by the SCC loop. Process them separately.
    let isolated: Vec<NodeId> = all_nodes
        .keys()
        .copied()
        .filter(|&id| !transclusions.contains_node(id))
        .collect();

    for id in isolated {
        if let Some(entry) = all_nodes.get_mut(&id) {
            let new_cache = backmatter_cache(id, &links, &transclusions, &outlinks_accumulator);
            entry.backmatter_cache = Some(new_cache);
            render_order.push(id);
        }
    }

    let site_context = minijinja::context! {
        root_directory => minijinja::Value::from_safe_string(config.root_directory.clone()),
        trailing_slash => config.trailing_slash,
        index_node => config.index_node.as_str(),
        domain => config.domain.as_str(),
    };
    let transclusion_template = config
        .environment
        .get_template(crate::config::TRANSCLUSION_TEMPLATE)
        .expect("bug: transclusion.html template missing");
    let link_template = config
        .environment
        .get_template(crate::config::LINK_TEMPLATE)
        .expect("bug: link.html template missing");
    let node_template = config
        .environment
        .get_template(crate::config::NODE_TEMPLATE)
        .expect("bug: node.html template missing");
    let backmatter_template = config
        .environment
        .get_template(crate::config::BACKMATTER_TEMPLATE)
        .expect("bug: backmatter.html template missing");

    for &id in &render_order {
        let rendered_body = render_body(
            id,
            &all_nodes,
            &interner,
            &link_template,
            &transclusion_template,
            config,
            &site_context,
        )?;
        all_nodes.get_mut(&id).unwrap().rendered_body = Some(rendered_body);
        let rendered_backmatter = render_backmatter(
            id,
            &all_nodes,
            &interner,
            &backmatter_template,
            config,
            &site_context,
        )?;
        all_nodes.get_mut(&id).unwrap().rendered_backmatter = Some(rendered_backmatter);
    }

    render_order
        .iter()
        .map(|&id| -> anyhow::Result<(String, String)> {
            let name = interner.name(id);
            let entry = &all_nodes[&id];
            let body = entry
                .rendered_body
                .as_deref()
                .expect("bug: no rendered_body after pass 2");
            let backmatter = entry
                .rendered_backmatter
                .as_deref()
                .expect("bug: no rendered_backmatter after pass 2");
            let html = render_node(
                name,
                entry,
                body,
                backmatter,
                &node_template,
                config,
                &site_context,
            )?;
            Ok((name.to_owned(), html))
        })
        .collect::<anyhow::Result<_>>()
}
