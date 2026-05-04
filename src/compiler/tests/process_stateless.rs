use std::collections::{HashMap, HashSet};

use ecow::EcoVec;
use petgraph::{
    Direction,
    algo::{tarjan_scc, toposort},
    prelude::DiGraphMap,
};
use typst::diag::Warned;
use typst_syntax::FileId;

use crate::compiler::{
    Backmatter, CompileDiagnostics, Node, NodeId, NodeInterner, ProcessDiagnostics,
    backmatter_input, body_input, cycle_diagnostics, dangling_link_diagnostic,
    dangling_transclusion_diagnostic, duplicate_node_identifier_diagnostic,
    extract::NodeOutput,
    node_input,
    render::Render,
    tests::{
        reference_compiler::State,
        render::{MockRenderer, RenderBackmatter, RenderBody, RenderNode},
    },
};

pub fn process_stateless(
    state: &State,
) -> anyhow::Result<(
    HashMap<String, RenderNode>,
    CompileDiagnostics,
    ProcessDiagnostics,
)> {
    // TODO: Get rid of NodeInterner in this implementation?

    let mut interner = NodeInterner::default();
    let mut links: DiGraphMap<NodeId, ()> = DiGraphMap::new();
    let mut transclusions: DiGraphMap<NodeId, ()> = DiGraphMap::new();
    let mut nodes: HashMap<NodeId, Node> = HashMap::new();
    let mut node_to_file: HashMap<NodeId, FileId> = HashMap::new();
    let mut compile_diagnostics: CompileDiagnostics = HashMap::new();
    let mut process_diagnostics: ProcessDiagnostics = HashMap::new();

    let mut occurrences: HashMap<NodeId, Vec<(FileId, NodeOutput)>> = HashMap::new();
    for &file_id in state.files.keys() {
        let Warned { output, warnings } = state.compile_file(file_id);

        match output {
            Ok(file_nodes) => {
                if !warnings.is_empty() {
                    compile_diagnostics.insert(file_id, (warnings, EcoVec::new()));
                }
                for node_output in file_nodes {
                    let node_id = interner.intern(node_output.identifier.as_str());
                    occurrences
                        .entry(node_id)
                        .or_default()
                        .push((file_id, node_output));
                }
            }
            Err(errors) => {
                compile_diagnostics.insert(file_id, (warnings, errors));
            }
        }
    }

    for (node_id, mut sources) in occurrences {
        assert!(!sources.is_empty());

        if sources.len() == 1 {
            let (file_id, output) = sources.pop().expect("len checked");
            for transclusion_id in output
                .transclusions
                .iter()
                .map(|t| interner.intern(t.as_str()))
            {
                transclusions.add_edge(node_id, transclusion_id, ());
            }
            for link_id in output.links.iter().map(|l| interner.intern(l.as_str())) {
                links.add_edge(node_id, link_id, ());
            }
            node_to_file.insert(node_id, file_id);
            nodes.insert(node_id, output.node);
        } else {
            let name = interner.name(node_id);
            for (file_id, _) in sources {
                process_diagnostics
                    .entry(file_id)
                    .or_default()
                    .push(duplicate_node_identifier_diagnostic(name));
            }
        }
    }

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

    // TODO: We're reusing the `*_input` utility functions in the test code,
    // which we're not catching any bugs in those helpers. We should probably
    // explicitly refactor this last code into a separate component which we
    // test on its own, similar to how we did for the extraction code.

    let renderer = MockRenderer;

    for &id in &render_order {
        let name = interner.name(id);

        let rendered_body = renderer.render_body(body_input(
            |id| nodes_helper(&nodes, id),
            &rendered_bodies,
            &interner,
            id,
        ))?;
        rendered_bodies.insert(id, rendered_body);

        let backmatter = collect_backmatter(id, &links, &transclusions);
        let rendered_backmatter = renderer.render_backmatter(backmatter_input(
            |id| nodes_helper(&nodes, id),
            &backmatter,
            &interner,
        ))?;
        rendered_backmatters.insert(id, rendered_backmatter);

        let rendered_node = renderer.render_node(node_input(
            |id| nodes_helper(&nodes, id),
            &rendered_bodies,
            &rendered_backmatters,
            &interner,
            id,
        ))?;

        output.insert(name.to_owned(), rendered_node);
    }

    return Ok((output, compile_diagnostics, process_diagnostics));

    fn nodes_helper(nodes: &HashMap<NodeId, Node>, node_id: NodeId) -> Option<&Node> {
        nodes.get(&node_id)
    }
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
