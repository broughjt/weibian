mod extract;
mod render;
#[cfg(test)]
mod tests;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io;

use ecow::{EcoVec, eco_format};
use petgraph::Direction;
use petgraph::algo::tarjan_scc;
use petgraph::graphmap::DiGraphMap;
use typst::World;
use typst::diag::{SourceDiagnostic, Warned};
use typst::syntax::{FileId, Span};

use self::extract::{compile_output, extract};
use self::render::{render_backmatter, render_body, render_node};
use crate::config::{
    BACKMATTER_TEMPLATE, BuildConfig, LINK_TEMPLATE, NODE_TEMPLATE, RenderConfig,
    TRANSCLUSION_TEMPLATE,
};

const HTML_MESSAGE: &str = "html export is under active development and incomplete";

/// Compiles Typst source files into nodes and maintains the in-memory node
/// store and per-file diagnostics across incremental rebuilds.
#[derive(Default)]
pub struct Compiler {
    file_to_nodes: HashMap<FileId, Vec<NodeId>>,
    node_to_file: HashMap<NodeId, FileId>,
    nodes: HashMap<NodeId, NodeEntry>,
    compile_diagnostics: HashMap<FileId, (EcoVec<SourceDiagnostic>, EcoVec<SourceDiagnostic>)>,
    process_diagnostics: HashMap<FileId, EcoVec<SourceDiagnostic>>,
    links: DiGraphMap<NodeId, ()>,
    transclusions: DiGraphMap<NodeId, ()>,
    interner: NodeInterner,
    dirty: HashSet<NodeId>,
    removed: HashSet<NodeId>,
    metadata_dirty: HashSet<NodeId>,
}

impl Compiler {
    /// Compiles a single source file and splits it into nodes, updating the
    /// node store and diagnostics.
    ///
    /// Typst compile errors and node-splitting errors (e.g. duplicate node IDs)
    /// are stored as diagnostics rather than returned as errors.
    pub fn update<C: Compile>(&mut self, compiler: &C, id: FileId) {
        let Warned {
            output: result,
            warnings,
        } = compiler.compile(id);

        // Exclude nodes belonging to this file from the duplicate check: they
        // are being replaced, not duplicated, and remove() hasn't been called
        // yet. The `extract` helper handles intra-file duplicates.
        let nodes_result = result.and_then(|output| {
            extract(output, &mut self.interner, |node_id| {
                self.nodes.contains_key(&node_id) && self.node_to_file.get(&node_id) != Some(&id)
            })
        });

        match nodes_result {
            Ok(nodes) => {
                // Compare new title/metadata against old entries before
                // remove() clears them, so we can track which nodes had their
                // displayable backmatter information change, adding them to
                // `metadata_dirty`.
                //
                // New nodes are not added to `metadata_dirty`: no existing
                // BackmatterCache can reference a node that didn't exist.
                for (&node_id, (entry, _, _)) in &nodes {
                    if self.nodes.get(&node_id).is_some_and(|old| {
                        old.title != entry.title || old.metadata != entry.metadata
                    }) {
                        self.metadata_dirty.insert(node_id);
                    }
                }

                // Now safe to orphan the old compilation.
                self.remove(id);

                for (&node_id, (_, transclusions, links)) in &nodes {
                    self.node_to_file.insert(node_id, id);
                    self.removed.remove(&node_id);
                    self.dirty.insert(node_id);

                    for &transclusion in transclusions {
                        self.transclusions.add_edge(node_id, transclusion, ());
                    }
                    for &link in links {
                        self.links.add_edge(node_id, link, ());
                    }
                }

                self.file_to_nodes
                    .insert(id, nodes.keys().copied().collect());
                // TODO: Either move this into the for loop above or move the
                // `node_to_file` to an extend down here.
                self.nodes
                    .extend(nodes.into_iter().map(|(k, (v, _, _))| (k, v)));

                if !warnings.is_empty() {
                    self.compile_diagnostics
                        .insert(id, (warnings, EcoVec::new()));
                }
            }
            Err(errors) => {
                self.remove(id);
                self.compile_diagnostics.insert(id, (warnings, errors));
            }
        }
    }

    /// Removes a source file's nodes and diagnostics from the in-memory store.
    ///
    /// Called when a source file is deleted from disk. The removed node IDs
    /// are accumulated in `self.removed` so that `process` can delete their
    /// output files.
    pub fn remove(&mut self, id: FileId) {
        if let Some(old_ids) = self.file_to_nodes.remove(&id) {
            for old_id in old_ids {
                self.node_to_file.remove(&old_id);
                self.nodes.remove(&old_id);
                self.dirty.remove(&old_id);
                self.metadata_dirty.remove(&old_id);
                self.removed.insert(old_id);

                clear_outgoing(&mut self.transclusions, old_id);
                clear_outgoing(&mut self.links, old_id);
            }
        }

        self.compile_diagnostics.remove(&id);
    }

    /// Returns true if any node produced by the file with the given ID exists
    /// in the current node store.
    ///
    /// Used in tests to check whether a node is present before querying it.
    #[cfg(test)]
    pub fn has_node(&self, id: std::num::NonZeroU16) -> bool {
        let node_id_str = format!("n{id}");
        self.interner
            .get(&node_id_str)
            .is_some_and(|node_id| self.nodes.contains_key(&node_id))
    }

    /// Returns all compile-time diagnostics, keyed by source [`FileId`].
    ///
    /// Each entry is a `(warnings, errors)` pair of [`SourceDiagnostic`] vecs.
    pub fn compile_diagnostics(
        &self,
    ) -> &HashMap<FileId, (EcoVec<SourceDiagnostic>, EcoVec<SourceDiagnostic>)> {
        &self.compile_diagnostics
    }

    /// Returns all structural diagnostics produced by the last [`Compiler::process`] call,
    /// keyed by source [`FileId`].
    ///
    /// These are errors detected across the full node graph (e.g. transclusion
    /// cycles) and are recomputed from scratch on every [`Compiler::process`]
    /// call.
    pub fn process_diagnostics(&self) -> &HashMap<FileId, EcoVec<SourceDiagnostic>> {
        &self.process_diagnostics
    }

    /// Returns an [`OutputPlan`] describing the writes and deletes to apply to
    /// the output directory, and clears the dirty and removed sets.
    pub fn process(&mut self, config: &RenderConfig) -> anyhow::Result<OutputPlan> {
        assert!(self.metadata_dirty.is_subset(&self.dirty));
        assert!(self.dirty.is_disjoint(&self.removed));

        if self.dirty.is_empty() && self.removed.is_empty() {
            return Ok(OutputPlan {
                writes: HashMap::new(),
                deletes: HashSet::new(),
            });
        }

        self.process_diagnostics.clear();

        // Check for dangling transclusions and links

        // TODO: Could possibly fold this into the render loop
        // Warn on dangling transclusions (target node does not exist).
        for (source, destination, _) in self
            .transclusions
            .all_edges()
            .filter(|&(_, destination, _)| !self.nodes.contains_key(&destination))
        {
            let file_id = *self
                .node_to_file
                .get(&source)
                .expect("bug: node in transclusion graph has no file entry");
            let name = self.interner.name(destination);
            self.process_diagnostics
                .entry(file_id)
                .or_default()
                .push(dangling_transclusion_diagnostic(name));
        }

        // Warn on dangling links (target node does not exist).
        for (source, destination, _) in self
            .links
            .all_edges()
            .filter(|&(_, destination, _)| !self.nodes.contains_key(&destination))
        {
            let file_id = *self
                .node_to_file
                .get(&source)
                .expect("bug: node in link graph has no file entry");
            let name = self.interner.name(destination);
            self.process_diagnostics
                .entry(file_id)
                .or_default()
                .push(dangling_link_diagnostic(name));
        }

        // Pass 1

        let dirty = std::mem::take(&mut self.dirty);
        let removed = std::mem::take(&mut self.removed);
        let metadata_dirty = std::mem::take(&mut self.metadata_dirty);

        let mut body_affected: HashSet<NodeId> = HashSet::new();
        let mut backmatter_affected: HashSet<NodeId> = HashSet::new();
        let mut unrenderable: HashSet<NodeId> = HashSet::new();
        let mut outlinks_accumulator: HashMap<NodeId, BTreeSet<NodeId>> = HashMap::new();
        let mut render_order: Vec<NodeId> = Vec::new();

        let sccs = tarjan_scc(&self.transclusions);

        for scc in &sccs {
            let id = scc[0];
            let is_cyclic = scc.len() > 1 || self.transclusions.contains_edge(id, id);

            if is_cyclic {
                // Treat the whole SCC atomically: if any member is dirty, or
                // any cross-SCC transclusion target is body_affected or
                // removed, every member is body_affected. A per-member linear
                // scan would be order-dependent and miss propagation across the
                // cycle (e.g. dirty node appears after its cycle partner).
                //
                // We only check cross-SCC edges because intra-SCC edges point
                // to nodes whose body_affected status we are currently deciding
                // — checking them would be circular.
                //
                // Removed nodes cannot appear in cycles: `remove()` clears all
                // outgoing edges, which breaks any cycle that passed through
                // the removed node. So we do not need to guard against removed
                // nodes being spuriously added to body_affected here.
                let scc_set: HashSet<NodeId> = scc.iter().copied().collect();
                let any_affected = scc.iter().any(|&m| dirty.contains(&m))
                    || scc
                        .iter()
                        .flat_map(|&m| self.transclusions.neighbors(m))
                        .filter(|t| !scc_set.contains(t))
                        .any(|t| body_affected.contains(&t) || removed.contains(&t));
                if any_affected {
                    body_affected.extend(scc.iter().copied());
                }

                unrenderable.extend(scc.iter().copied());

                // TODO: Store spans in the compiler state so we can
                // point out the file locations of the offending
                // transclusions
                for (file_id, diag) in cycle_diagnostics(scc.iter().map(|&id| {
                    let file_id = *self
                        .node_to_file
                        .get(&id)
                        .expect("bug: node in transclusion cycle has no file entry");
                    (file_id, self.interner.name(id))
                })) {
                    self.process_diagnostics
                        .entry(file_id)
                        .or_default()
                        .push(diag);
                }
            } else {
                // Non-cyclic SCCs are always singletons, so a per-node check
                // is unambiguous — no ordering concern.
                //
                // Removed nodes cannot be spuriously added to body_affected
                // here: remove() clears outgoing edges, so
                // transclusions.neighbors returns empty for them, and they are
                // not in dirty. Both conditions are therefore false.
                let is_body_affected = dirty.contains(&id)
                    || self
                        .transclusions
                        .neighbors(id)
                        .any(|t| body_affected.contains(&t) || removed.contains(&t));
                if is_body_affected {
                    body_affected.insert(id);
                }

                if self
                    .transclusions
                    .neighbors(id)
                    .any(|t| unrenderable.contains(&t))
                {
                    unrenderable.insert(id);
                } else {
                    // Dangling transclusions stay in the transclusion graph
                    // until the transclusion is removed. We cannot assume they
                    // are in the removed set, since they might have been
                    // dangling in many previous calls to `process`. We check
                    // for dangling transclusions in a loop above.
                    if let Some(entry) = self.nodes.get_mut(&id) {
                        let new_cache = backmatter_cache(
                            id,
                            &self.links,
                            &self.transclusions,
                            &outlinks_accumulator,
                        );
                        outlinks_accumulator.insert(id, new_cache.outlinks.clone());

                        if should_backmatter_render(
                            entry.backmatter_cache.as_ref(),
                            &new_cache,
                            &metadata_dirty,
                            &removed,
                        ) {
                            entry.backmatter_cache = Some(new_cache);
                            backmatter_affected.insert(id);
                        }
                    }

                    if body_affected.contains(&id) || backmatter_affected.contains(&id) {
                        render_order.push(id);
                    }
                }
            }
        }
        for (&id, entry) in self
            .nodes
            .iter_mut()
            .filter(|(id, _)| !self.transclusions.contains_node(**id))
        {
            if dirty.contains(&id) {
                body_affected.insert(id);
            }

            let new_cache =
                backmatter_cache(id, &self.links, &self.transclusions, &outlinks_accumulator);

            if should_backmatter_render(
                entry.backmatter_cache.as_ref(),
                &new_cache,
                &metadata_dirty,
                &removed,
            ) {
                entry.backmatter_cache = Some(new_cache);
                backmatter_affected.insert(id);
            }

            if body_affected.contains(&id) || backmatter_affected.contains(&id) {
                render_order.push(id);
            }
        }

        // Pass 2: render nodes in order (isolated first, then leaves-to-roots).

        let site_context = minijinja::context! {
            root_directory => minijinja::Value::from_safe_string(config.root_directory.clone()),
            trailing_slash => config.trailing_slash,
            index_node => config.index_node.as_str(),
            domain => config.domain.as_str(),
        };

        let transclusion_template = config
            .environment
            .get_template(TRANSCLUSION_TEMPLATE)
            .expect("bug: transclusion.html template missing from environment");
        let link_template = config
            .environment
            .get_template(LINK_TEMPLATE)
            .expect("bug: link.html template missing from environment");
        let node_template = config
            .environment
            .get_template(NODE_TEMPLATE)
            .expect("bug: node.html template missing from environment");
        let backmatter_template = config
            .environment
            .get_template(BACKMATTER_TEMPLATE)
            .expect("bug: backmatter.html template missing from environment");

        for &id in &render_order {
            if body_affected.contains(&id) {
                let rendered_body = render_body(
                    id,
                    &self.nodes,
                    &self.interner,
                    &link_template,
                    &transclusion_template,
                    config,
                    &site_context,
                )?;
                self.nodes.get_mut(&id).unwrap().rendered_body = Some(rendered_body);
            }
            if backmatter_affected.contains(&id) {
                let rendered_backmatter = render_backmatter(
                    id,
                    &self.nodes,
                    &self.interner,
                    &backmatter_template,
                    config,
                    &site_context,
                )?;
                self.nodes.get_mut(&id).unwrap().rendered_backmatter = Some(rendered_backmatter);
            }
        }

        let writes = render_order
            .iter()
            .map(|&id| -> anyhow::Result<(String, String)> {
                let name = self.interner.name(id);
                let entry = &self.nodes[&id];
                let body = entry
                    .rendered_body
                    .as_deref()
                    .expect("bug: renderable node has no rendered_body after pass 2");
                let backmatter = entry
                    .rendered_backmatter
                    .as_deref()
                    .expect("bug: renderable node has no rendered backmatter after pass 2");
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
        let deletes = removed
            .iter()
            .chain(unrenderable.intersection(&body_affected))
            .map(|&id| self.interner.name(id).to_string())
            .collect();

        Ok(OutputPlan { writes, deletes })
    }
}

/// The output of a successful file compilation.
pub struct CompileOutput {
    /// The HTML body of the compiled file.
    pub html: String,
    /// Spans for each node identifier within the document, used for diagnostic reporting.
    pub spans: HashMap<String, Span>,
    /// Node metadata keyed by node identifier.
    pub metadata: HashMap<String, HashMap<String, Vec<String>>>,
    /// Transclusion metadata keyed by counter.
    pub transclusion_metadata: HashMap<u32, HashMap<String, Vec<String>>>,
    /// Link metadata keyed by counter.
    pub link_metadata: HashMap<u32, HashMap<String, Vec<String>>>,
    /// Diagnostics collected during span and metadata extraction.
    pub errors: EcoVec<SourceDiagnostic>,
}

/// Compiles a source file into [`CompileOutput`].
///
/// The `id` parameter identifies which file is being compiled. Implementations
/// backed by a Typst [`World`] may ignore it since the world already encodes
/// the target file; test implementations use it to look up canned output.
pub trait Compile {
    fn compile(&self, id: FileId) -> Warned<Result<CompileOutput, EcoVec<SourceDiagnostic>>>;
}

/// Wraps a Typst [`World`] so it can be passed to [`Compiler::update`].
pub struct TypstCompile<W>(pub W);

impl<W: World> Compile for TypstCompile<W> {
    fn compile(&self, _id: FileId) -> Warned<Result<CompileOutput, EcoVec<SourceDiagnostic>>> {
        compile_output(&self.0)
    }
}

#[derive(Copy, Clone, Hash, Eq, PartialEq, Ord, PartialOrd, Debug)]
pub struct NodeId(u32);

/// Interns node name strings to compact [`NodeId`] handles.
#[derive(Default)]
pub struct NodeInterner {
    forward: HashMap<String, NodeId>,
    reverse: Vec<String>,
}

impl NodeInterner {
    /// Returns the [`NodeId`] for `name`, interning it if not already present.
    pub fn intern<S: Into<String> + AsRef<str>>(&mut self, name: S) -> NodeId {
        if let Some(&id) = self.forward.get(name.as_ref()) {
            return id;
        }
        let id = NodeId(self.reverse.len() as u32);
        let name_owned = name.into();
        self.forward.insert(name_owned.clone(), id);
        self.reverse.push(name_owned);
        id
    }

    /// Returns the [`NodeId`] for `name` if it has been interned.
    pub fn get(&self, name: &str) -> Option<NodeId> {
        self.forward.get(name).copied()
    }

    /// Returns the name string for a [`NodeId`].
    ///
    /// Panics if `id` was not produced by this interner.
    pub fn name(&self, id: NodeId) -> &str {
        &self.reverse[id.0 as usize]
    }
}

/// Cached backmatter sets for a node, used to determine whether backmatter
/// needs to be re-rendered on the next [`Compiler::process`] call.
#[derive(PartialEq, Eq)]
pub struct BackmatterCache {
    pub contexts: BTreeSet<NodeId>,
    pub backlinks: BTreeSet<NodeId>,
    pub outlinks: BTreeSet<NodeId>,
}

pub struct NodeEntry {
    pub raw_html: String,
    pub title: String,
    pub title_text: String,
    pub span: Span,
    // TODO: Should we intern metadata strings and output? Is that nuts? Would
    // it cause incorrectness?
    pub metadata: HashMap<String, Vec<String>>,
    pub transclusion_metadata: HashMap<u32, HashMap<String, Vec<String>>>,
    pub link_metadata: HashMap<u32, HashMap<String, Vec<String>>>,
    pub rendered_body: Option<String>,
    pub rendered_backmatter: Option<String>,
    pub backmatter_cache: Option<BackmatterCache>,
}

impl Default for NodeEntry {
    fn default() -> Self {
        Self {
            raw_html: String::new(),
            title: String::new(),
            title_text: String::new(),
            span: Span::detached(),
            metadata: HashMap::new(),
            transclusion_metadata: HashMap::new(),
            link_metadata: HashMap::new(),
            rendered_body: None,
            rendered_backmatter: None,
            backmatter_cache: None,
        }
    }
}

/// The set of writes and deletes to apply to the output directory after a
/// process call.
/// Removes all outgoing edges from `id` in `graph`, leaving incoming edges
/// (and thus transitive dependents) intact.
fn clear_outgoing(graph: &mut DiGraphMap<NodeId, ()>, id: NodeId) {
    let neighbors: Vec<NodeId> = graph.neighbors(id).collect();
    for neighbor in neighbors {
        graph.remove_edge(id, neighbor);
    }
}

// TODO: Future improvement: We have spans, we should not use detached for these diagnostics

fn dangling_transclusion_diagnostic(name: &str) -> SourceDiagnostic {
    SourceDiagnostic::warning(
        Span::detached(),
        eco_format!("dangling transclusion: {name} is not defined"),
    )
}

fn dangling_link_diagnostic(name: &str) -> SourceDiagnostic {
    SourceDiagnostic::warning(
        Span::detached(),
        eco_format!("dangling link: {name} is not defined"),
    )
}

fn cycle_diagnostics<'a>(
    pairs: impl Iterator<Item = (FileId, &'a str)>,
) -> Vec<(FileId, SourceDiagnostic)> {
    let (files, names): (HashSet<FileId>, Vec<&str>) = pairs.unzip();
    let message = eco_format!("transclusion cycle: {}", names.join(", "));

    files
        .into_iter()
        .map(|file_id| {
            (
                file_id,
                SourceDiagnostic::error(Span::detached(), message.clone()),
            )
        })
        .collect()
}

fn backmatter_cache(
    id: NodeId,
    links: &DiGraphMap<NodeId, ()>,
    transclusions: &DiGraphMap<NodeId, ()>,
    outlinks_accumulator: &HashMap<NodeId, BTreeSet<NodeId>>,
) -> BackmatterCache {
    let mut outlinks: BTreeSet<NodeId> = links.neighbors(id).collect();
    for target in transclusions.neighbors(id) {
        if let Some(target_outlinks) = outlinks_accumulator.get(&target) {
            outlinks.extend(target_outlinks.iter());
        }
    }
    let contexts: BTreeSet<NodeId> = transclusions
        .neighbors_directed(id, Direction::Incoming)
        .collect();
    let backlinks: BTreeSet<NodeId> = links.neighbors_directed(id, Direction::Incoming).collect();

    BackmatterCache {
        contexts,
        backlinks,
        outlinks,
    }
}

fn should_backmatter_render(
    option_old: Option<&BackmatterCache>,
    new: &BackmatterCache,
    metadata_dirty: &HashSet<NodeId>,
    removed: &HashSet<NodeId>,
) -> bool {
    option_old.is_none_or(|old| {
        old != new
            || new
                .contexts
                .iter()
                .chain(new.backlinks.iter())
                .chain(new.outlinks.iter())
                .any(|id| metadata_dirty.contains(id) || removed.contains(id))
    })
}

pub struct OutputPlan {
    pub writes: HashMap<String, String>,
    pub deletes: HashSet<String>,
}

impl OutputPlan {
    pub fn apply(&self, config: &BuildConfig) -> Result<(), io::Error> {
        for (node_id, html) in &self.writes {
            let path = config.output_path(node_id);

            if let Some(parent) = path.parent()
                && parent != config.output_directory
            {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, html)?;
        }
        for node_id in &self.deletes {
            let path = config.output_path(node_id);

            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    eprintln!("warning: expected to delete {path:?} but it was not found");
                }
                Err(error) => return Err(error),
            }
        }

        Ok(())
    }
}

type ExtractOutput = (NodeEntry, Vec<NodeId>, Vec<NodeId>);
