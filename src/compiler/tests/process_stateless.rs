use std::{
    collections::{HashMap, HashSet},
    num::NonZeroU16,
};

use petgraph::{
    Direction,
    algo::{tarjan_scc, toposort},
    prelude::DiGraphMap,
};
use typst_syntax::FileId;

use crate::compiler::{
    Backmatter, CompileDiagnostics, NodeEntry, NodeId, NodeInterner, ProcessDiagnostics,
    build_backmatter_input, build_body_input, build_node_input, cycle_diagnostics,
    dangling_link_diagnostic, dangling_transclusion_diagnostic,
    extract::NodeOutput,
    render::{BodyInput, Render},
    tests::{
        reference_compiler::MockNode,
        render::{MockRenderer, RenderBackmatter, RenderBody, RenderNode},
    },
};

// TODO: Needs to be updated to fit the new shape of the reference compiler state.
pub fn process_stateless(
    files: &HashMap<NonZeroU16, HashMap<String, MockNode>>,
) -> anyhow::Result<(
    HashMap<String, RenderNode>,
    CompileDiagnostics,
    ProcessDiagnostics,
)> {
    // TODO: Get rid of NodeInterner in this implementation?

    let mut interner = NodeInterner::default();
    let mut links: DiGraphMap<NodeId, ()> = DiGraphMap::new();
    let mut transclusions: DiGraphMap<NodeId, ()> = DiGraphMap::new();
    let mut nodes: HashMap<NodeId, NodeEntry> = HashMap::new();
    let mut node_to_file: HashMap<NodeId, FileId> = HashMap::new();
    let compile_diagnostics: CompileDiagnostics = HashMap::new();

    for (&raw_file_id, file_nodes) in files {
        let file_id = FileId::from_raw(raw_file_id);

        for (identifier, mock_node) in file_nodes {
            let node_output = NodeOutput::from(mock_node.clone());
            let node_id = interner.intern(identifier.as_str());

            for t in &node_output.transclusions {
                let transclusion_id = interner.intern(t.as_str());
                transclusions.add_edge(node_id, transclusion_id, ());
            }
            for l in &node_output.links {
                let link_id = interner.intern(l.as_str());
                links.add_edge(node_id, link_id, ());
            }

            node_to_file.insert(node_id, file_id);
            nodes.insert(node_id, node_output.entry);
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
            for (file_id, diagnostic) in cycle_diagnostics(pairs) {
                process_diagnostics
                    .entry(file_id)
                    .or_default()
                    .push(diagnostic);
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
    let mut rendered_bodies: HashMap<NodeId, RenderBody> = HashMap::new();
    let mut rendered_backmatters: HashMap<NodeId, RenderBackmatter> = HashMap::new();

    // TODO: We're reusing the `build_*` utility functions in the test code,
    // which we're not catching any bugs in those helpers. We should probably
    // explicitly refactor this last code into a separate component which we
    // test on its own, similar to how we did for the extraction code.

    let renderer = MockRenderer;

    for &id in &render_order {
        let name = interner.name(id);
        let entry = &nodes[&id];

        let rendered_body =
            renderer.render_body(build_body_input(id, &nodes, &rendered_bodies, &interner))?;
        rendered_bodies.insert(id, rendered_body);

        let backmatter = collect_backmatter(id, &links, &transclusions);
        let rendered_backmatter = renderer.render_backmatter(build_backmatter_input(
            id,
            &nodes,
            &backmatter,
            &interner,
        ))?;
        rendered_backmatters.insert(id, rendered_backmatter);

        let rendered_node = renderer.render_node(build_node_input(
            id,
            &nodes,
            &rendered_bodies,
            &rendered_backmatters,
            &interner,
        ))?;

        output.insert(name.to_owned(), rendered_node);
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
