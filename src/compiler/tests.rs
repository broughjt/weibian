mod mock;
mod stateless;

use std::collections::{HashMap, HashSet};
use std::num::NonZeroU16;

use proptest::prelude::*;
use typst::syntax::FileId;

use crate::compiler::{Compiler, OutputPlan};
use crate::config::{
    BACKMATTER_TEMPLATE, LINK_TEMPLATE, NODE_TEMPLATE, RenderConfig, TRANSCLUSION_TEMPLATE,
};

use self::mock::{Event, EventConfig, MockNode, arbitrary_event_batches, arbitrary_events};
use self::stateless::process_stateless;

proptest! {
    #[test]
    fn compile_scratch_equal_compile_incremental_batched(
        batches in arbitrary_event_batches(&EventConfig::from_environment())
    ) {
        let config = render_config();

        let scratch = {
            let mut compiler = Compiler::default();
            for (id, node) in reduce_events(batches.iter().flatten()) {
                compiler.update(node, FileId::from_raw(id));
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
                        Event::Update(id, node) => compiler.update(node, FileId::from_raw(*id)),
                        Event::Remove(id) => compiler.remove(FileId::from_raw(*id)),
                    }
                }
                apply(compiler.process(&config).unwrap(), &mut files);
            }
            files
        };

        prop_assert_eq!(scratch, incremental);
    }

    #[test]
    fn compile_scratch_equal_compile_incremental(
        events in arbitrary_events(&EventConfig::from_environment())
    ) {
        let config = render_config();

        let scratch = {
            let mut compiler = Compiler::default();
            for (id, node) in reduce_events(events.iter()) {
                compiler.update(node, FileId::from_raw(id));
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
                    Event::Update(id, node) => compiler.update(node, FileId::from_raw(*id)),
                    Event::Remove(id) => compiler.remove(FileId::from_raw(*id)),
                }
                apply(compiler.process(&config).unwrap(), &mut files);
            }
            files
        };

        prop_assert_eq!(scratch, incremental);
    }

    #[test]
    fn output_plan_writes_and_deletes_are_disjoint(
        events in arbitrary_events(&EventConfig::from_environment())
    ) {
        let config = render_config();
        let mut compiler = Compiler::default();

        for event in &events {
            match event {
                Event::Update(id, node) => compiler.update(node, FileId::from_raw(*id)),
                Event::Remove(id) => compiler.remove(FileId::from_raw(*id)),
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
                Event::Update(id, node) => compiler.update(node, FileId::from_raw(*id)),
                Event::Remove(id) => compiler.remove(FileId::from_raw(*id)),
            }
        }
        compiler.process(&config).unwrap();

        let second = compiler.process(&config).unwrap();
        prop_assert!(second.writes.is_empty(), "expected no writes on second process call, got {:?}", second.writes.keys().collect::<Vec<_>>());
        prop_assert!(second.deletes.is_empty(), "expected no deletes on second process call, got {:?}", second.deletes);
    }

    // TODO:
    // #[test]
    // fn metadata_change_triggers_backmatter_rerender(
    //     events in arbitrary_events(&EventConfig::from_environment()),
    //     changed_id in any::<NonZeroU16>(),
    // ) {
    //     let config = render_config();
    //     let mut compiler = Compiler::default();

    //     for event in &events {
    //         match event {
    //             Event::Update(id, node) => compiler.update(node, FileId::from_raw(*id)),
    //             Event::Remove(id) => compiler.remove(FileId::from_raw(*id)),
    //         }
    //     }
    //     let first = compiler.process(&config).unwrap();
    //     let mut files: HashMap<String, String> = HashMap::new();
    //     apply(first, &mut files);

    //     // Emit a title-only update for changed_id (no edge changes).
    //     let node_id_str = format_id(changed_id);
    //     let changed_file = FileId::from_raw(changed_id);
    //     if !compiler.has_node(changed_id) {
    //         // Node doesn't exist yet; insert a fresh one with no edges.
    //         let node = MockNode { id: changed_id, title: "before".into(), body: String::new(), transcludes: vec![], links: vec![] };
    //         compiler.update(&node, changed_file);
    //         compiler.process(&config).unwrap();
    //     }

    //     // Update with a new title, keeping edges identical.
    //     let new_node = MockNode { id: changed_id, title: "after".into(), body: String::new(), transcludes: vec![], links: vec![] };
    //     compiler.update(&new_node, changed_file);
    //     let second = compiler.process(&config).unwrap();

    //     // changed_id itself must be rewritten (body dirty).
    //     if second.writes.contains_key(&node_id_str) || files.contains_key(&node_id_str) {
    //         prop_assert!(second.writes.contains_key(&node_id_str),
    //             "node {node_id_str} was not rewritten after title change");
    //     }

    //     // Every node whose rendered output previously contained changed_id in
    //     // its backmatter must be rewritten.
    //     for (name, old_html) in &files {
    //         let references_changed =
    //             old_html.contains(&format!("<ctx>{node_id_str}</ctx>"))
    //             || old_html.contains(&format!("<bl>{node_id_str}</bl>"))
    //             || old_html.contains(&format!("<ol>{node_id_str}</ol>"));
    //         if references_changed {
    //             prop_assert!(
    //                 second.writes.contains_key(name.as_str()),
    //                 "node {name} references {node_id_str} in backmatter but was not rewritten"
    //             );
    //         }
    //     }
    // }

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
                    incremental.update(node, FileId::from_raw(*id));
                    current_nodes.insert(FileId::from_raw(*id), node.clone());
                }
                Event::Remove(id) => {
                    incremental.remove(FileId::from_raw(*id));
                    current_nodes.remove(&FileId::from_raw(*id));
                }
            }
            apply(incremental.process(&config).unwrap(), &mut inc_fs);
            let (ref_fs, ..) = process_stateless(
                &current_nodes.values().cloned().collect::<Vec<_>>(),
                &config,
            )
            .unwrap();
            prop_assert_eq!(&inc_fs, &ref_fs);
        }
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
