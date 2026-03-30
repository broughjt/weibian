use std::collections::{HashMap, HashSet};

use ecow::EcoVec;
use typst::diag::{SourceDiagnostic, Warned};
use typst::syntax::FileId;

use crate::compiler::{Compile, NodeEntry, NodeId, NodeInterner};
use crate::config::RenderConfig;

use super::super::{
    backmatter_cache, cycle_diagnostics, dangling_link_diagnostic,
    dangling_transclusion_diagnostic, extract, render_backmatter, render_body, render_node,
};
use super::mock::MockNode;

type CompileDiagnostics = HashMap<FileId, (EcoVec<SourceDiagnostic>, EcoVec<SourceDiagnostic>)>;
type ProcessDiagnostics = HashMap<FileId, EcoVec<SourceDiagnostic>>;

/// Stateless reference implementation: compiles `mock_nodes` from scratch and
/// returns the complete rendered filesystem, compile diagnostics, and process
/// diagnostics.
///
/// Has no notion of previous state — no dirty/removed/metadata_dirty sets.
/// Every call recomputes everything. Used as a test oracle against the
/// incremental `Compiler`.
pub(super) fn process_stateless(
    mock_nodes: &[MockNode],
    config: &RenderConfig,
) -> anyhow::Result<(
    HashMap<String, String>,
    CompileDiagnostics,
    ProcessDiagnostics,
)> {
    use std::collections::BTreeSet;

    use petgraph::algo::tarjan_scc;
    use petgraph::graphmap::DiGraphMap;

    let mut interner = NodeInterner::default();
    let mut links: DiGraphMap<NodeId, ()> = DiGraphMap::new();
    let mut transclusions: DiGraphMap<NodeId, ()> = DiGraphMap::new();
    let mut nodes: HashMap<NodeId, NodeEntry> = HashMap::new();
    let mut node_to_file: HashMap<NodeId, FileId> = HashMap::new();
    let mut compile_diagnostics: CompileDiagnostics = HashMap::new();

    for mock_node in mock_nodes {
        let file_id = FileId::from_raw(mock_node.id);
        let Warned {
            output: result,
            warnings,
        } = mock_node.compile(file_id);

        // TODO: This correctly rejects cross-file duplicate node names, but
        // `process_stateless` processes files in HashMap iteration order while
        // the incremental compiler respects update() call order. These can
        // disagree on which file "wins" when a duplicate exists. This doesn't
        // matter currently because each MockNode produces exactly one node
        // whose name is derived from its own file ID, making cross-file
        // duplicates impossible. Revisit if subnodes are added to the mock
        // test data.
        match result.and_then(|output| {
            extract(output, &mut interner, |node_id| {
                nodes.contains_key(&node_id)
            })
        }) {
            Ok(extracted) => {
                for (node_id, (entry, trans, lnks)) in extracted {
                    node_to_file.insert(node_id, file_id);
                    for &t in &trans {
                        transclusions.add_edge(node_id, t, ());
                    }
                    for &l in &lnks {
                        links.add_edge(node_id, l, ());
                    }
                    nodes.insert(node_id, entry);
                }

                if !warnings.is_empty() {
                    compile_diagnostics.insert(file_id, (warnings, EcoVec::new()));
                }
            }
            Err(errors) => {
                compile_diagnostics.insert(file_id, (warnings, errors));
            }
        }
    }

    let mut process_diagnostics: ProcessDiagnostics = HashMap::new();

    for (source, destination, _) in transclusions
        .all_edges()
        .filter(|&(_, dst, _)| !nodes.contains_key(&dst))
    {
        let fid = *node_to_file
            .get(&source)
            .expect("bug: node in transclusion graph has no file entry");
        let name = interner.name(destination);
        process_diagnostics
            .entry(fid)
            .or_default()
            .push(dangling_transclusion_diagnostic(name));
    }

    for (source, destination, _) in links
        .all_edges()
        .filter(|&(_, dst, _)| !nodes.contains_key(&dst))
    {
        let fid = *node_to_file
            .get(&source)
            .expect("bug: node in link graph has no file entry");
        let name = interner.name(destination);
        process_diagnostics
            .entry(fid)
            .or_default()
            .push(dangling_link_diagnostic(name));
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
            for (fid, diag) in cycle_diagnostics(scc.iter().map(|&id| {
                let fid = *node_to_file
                    .get(&id)
                    .expect("bug: node in transclusion cycle has no file entry");
                (fid, interner.name(id))
            })) {
                process_diagnostics.entry(fid).or_default().push(diag);
            }
        } else if transclusions
            .neighbors(id)
            .any(|t| unrenderable.contains(&t))
        {
            unrenderable.insert(id);
        } else if let Some(entry) = nodes.get_mut(&id) {
            let new_cache = backmatter_cache(id, &links, &transclusions, &outlinks_accumulator);
            outlinks_accumulator.insert(id, new_cache.outlinks.clone());
            entry.backmatter_cache = Some(new_cache);
            render_order.push(id);
        }
    }

    // Nodes that appear in no transclusion edge (neither source nor target) are
    // not visited by the SCC loop. Process them separately.
    let isolated: Vec<NodeId> = nodes
        .keys()
        .copied()
        .filter(|&id| !transclusions.contains_node(id))
        .collect();

    for id in isolated {
        if let Some(entry) = nodes.get_mut(&id) {
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
            &nodes,
            &interner,
            &link_template,
            &transclusion_template,
            config,
            &site_context,
        )?;
        nodes.get_mut(&id).unwrap().rendered_body = Some(rendered_body);
        let rendered_backmatter = render_backmatter(
            id,
            &nodes,
            &interner,
            &backmatter_template,
            config,
            &site_context,
        )?;
        nodes.get_mut(&id).unwrap().rendered_backmatter = Some(rendered_backmatter);
    }

    let fs = render_order
        .iter()
        .map(|&id| -> anyhow::Result<(String, String)> {
            let name = interner.name(id);
            let entry = &nodes[&id];
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
        .collect::<anyhow::Result<_>>()?;

    Ok((fs, compile_diagnostics, process_diagnostics))
}
