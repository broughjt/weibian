use std::collections::{HashMap, HashSet};

use ecow::EcoVec;
use petgraph::Direction;
use petgraph::algo::{tarjan_scc, toposort};
use petgraph::prelude::DiGraphMap;
use typst::diag::Warned;
use typst::syntax::FileId;

use crate::compiler::{
    Backmatter, Compile, CompileDiagnostics, NodeEntry, NodeId, NodeInterner, ProcessDiagnostics,
};
use crate::config::RenderConfig;

use super::super::{
    Renderer, cycle_diagnostics, dangling_link_diagnostic, dangling_transclusion_diagnostic,
    extract,
};
use super::mock::MockFile;

/// Stateless reference implementation: compiles `mock_nodes` from scratch and
/// returns the complete rendered filesystem, compile diagnostics, and process
/// diagnostics.
///
/// Has no notion of previous state — no dirty/removed/metadata_dirty sets.
/// Every call recomputes everything. Used as a test oracle against the
/// incremental `Compiler`.
pub(super) fn process_stateless(
    ordered_files: &[(FileId, MockFile)],
    config: &RenderConfig,
) -> anyhow::Result<(
    HashMap<String, String>,
    CompileDiagnostics,
    ProcessDiagnostics,
)> {
    let mut interner = NodeInterner::default();
    let mut links: DiGraphMap<NodeId, ()> = DiGraphMap::new();
    let mut transclusions: DiGraphMap<NodeId, ()> = DiGraphMap::new();
    let mut nodes: HashMap<NodeId, NodeEntry> = HashMap::new();
    let mut node_to_file: HashMap<NodeId, FileId> = HashMap::new();
    let mut compile_diagnostics: CompileDiagnostics = HashMap::new();

    for (file_id, mock_file) in ordered_files {
        let Warned {
            output: result,
            warnings,
        } = mock_file.compile();

        match result.and_then(|output| {
            extract(output, &mut interner, |node_id| {
                nodes.contains_key(&node_id)
            })
        }) {
            Ok(extracted) => {
                for (id, (entry, ts, ls)) in extracted {
                    node_to_file.insert(id, *file_id);
                    for &t in &ts {
                        transclusions.add_edge(id, t, ());
                    }
                    for &l in &ls {
                        links.add_edge(id, l, ());
                    }
                    nodes.insert(id, entry);
                }

                if !warnings.is_empty() {
                    compile_diagnostics.insert(*file_id, (warnings, EcoVec::new()));
                }
            }
            Err(errors) => {
                compile_diagnostics.insert(*file_id, (warnings, errors));
            }
        }
    }

    let mut process_diagnostics: ProcessDiagnostics = HashMap::new();

    for (source, destination, _) in transclusions
        .all_edges()
        .filter(|&(_, dst, _)| !nodes.contains_key(&dst))
    {
        let file_id = *node_to_file
            .get(&source)
            .expect("bug: node in transclusion graph has no file entry");
        let name = interner.name(destination);
        process_diagnostics
            .entry(file_id)
            .or_default()
            .push(dangling_transclusion_diagnostic(name));
    }
    for (source, destination, _) in links
        .all_edges()
        .filter(|&(_, dst, _)| !nodes.contains_key(&dst))
    {
        let file_id = *node_to_file
            .get(&source)
            .expect("bug: node in link graph has no file entry");
        let name = interner.name(destination);
        process_diagnostics
            .entry(file_id)
            .or_default()
            .push(dangling_link_diagnostic(name));
    }

    let mut cyclic_nodes: HashSet<NodeId> = HashSet::new();
    for scc in tarjan_scc(&transclusions) {
        let id = scc[0];
        if scc.len() > 1 || transclusions.contains_edge(id, id) {
            cyclic_nodes.extend(scc.iter());

            let pairs = scc.iter().map(|&id| {
                let file_id = *node_to_file
                    .get(&id)
                    .expect("bug: node in transclusion cycle has no file entry");

                (file_id, interner.name(id))
            });
            for (file_id, diag) in cycle_diagnostics(pairs) {
                process_diagnostics.entry(file_id).or_default().push(diag);
            }
        }
    }

    let mut unrenderable = cyclic_nodes.clone();
    let mut stack: Vec<NodeId> = cyclic_nodes.into_iter().collect();
    while let Some(id) = stack.pop() {
        for source in transclusions.neighbors_directed(id, Direction::Incoming) {
            if unrenderable.insert(source) {
                stack.push(source);
            }
        }
    }

    let renderable: HashSet<NodeId> = nodes
        .keys()
        .copied()
        .filter(|id| !unrenderable.contains(id))
        .collect();

    let mut renderable_transclusions: DiGraphMap<NodeId, ()> = DiGraphMap::new();
    for &id in &renderable {
        renderable_transclusions.add_node(id);
    }
    for (source, destination, _) in transclusions.all_edges() {
        if renderable.contains(&source) && renderable.contains(&destination) {
            renderable_transclusions.add_edge(source, destination, ());
        }
    }

    let render_order: Vec<NodeId> = toposort(&renderable_transclusions, None)
        .expect("bug: renderable stateless transclusion graph must be acyclic")
        .into_iter()
        .rev()
        .collect();

    let mut output = HashMap::new();
    let mut rendered_bodies: HashMap<NodeId, String> = HashMap::new();
    let mut rendered_backmatters: HashMap<NodeId, String> = HashMap::new();
    let renderer = Renderer::new(&nodes, &interner, config);

    for &id in &render_order {
        let backmatter = collect_backmatter(id, &links, &transclusions);

        let rendered_body = renderer.render_body(id, &rendered_bodies)?;
        rendered_bodies.insert(id, rendered_body);

        let rendered_backmatter = renderer.render_backmatter(id, &backmatter)?;
        rendered_backmatters.insert(id, rendered_backmatter);

        let name = interner.name(id);
        let entry = &nodes[&id];
        let html = renderer.render_node(
            name,
            entry,
            rendered_bodies
                .get(&id)
                .map(String::as_str)
                .expect("bug: stateless render body missing after render_body"),
            rendered_backmatters
                .get(&id)
                .map(String::as_str)
                .expect("bug: stateless rendered backmatter missing after render_backmatter"),
        )?;

        output.insert(name.to_owned(), html);
    }

    Ok((output, compile_diagnostics, process_diagnostics))
}

fn collect_backmatter(
    id: NodeId,
    links: &DiGraphMap<NodeId, ()>,
    transclusions: &DiGraphMap<NodeId, ()>,
) -> Backmatter {
    Backmatter {
        contexts: transclusions
            .neighbors_directed(id, Direction::Incoming)
            .collect(),
        backlinks: links.neighbors_directed(id, Direction::Incoming).collect(),
        outlinks: collect_outlinks(id, links, transclusions),
    }
}

fn collect_outlinks(
    id: NodeId,
    links: &DiGraphMap<NodeId, ()>,
    transclusions: &DiGraphMap<NodeId, ()>,
) -> HashSet<NodeId> {
    let mut visited: HashSet<NodeId> = HashSet::new();
    let mut stack = vec![id];
    let mut outlinks = HashSet::new();

    while let Some(current) = stack.pop() {
        if !visited.insert(current) {
            continue;
        }

        outlinks.extend(links.neighbors(current));
        stack.extend(transclusions.neighbors(current));
    }

    outlinks
}
