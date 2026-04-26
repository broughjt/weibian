mod extract;
mod render;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::io;

use dom_query::Document;
use ecow::{EcoVec, eco_format};
use petgraph::Direction;
use petgraph::algo::tarjan_scc;
use petgraph::graphmap::DiGraphMap;
use typst::World;
use typst::diag::{SourceDiagnostic, Warned};
use typst::syntax::{FileId, Span};

use self::extract::NodeOutput;
use self::render::{
    BackmatterInput, BackmatterNode, BodyInput, JinjaRenderer, LinkInput, NodeInput, Render,
    ResolvedLink, ResolvedTransclusion, TransclusionInput,
};
use crate::config::{BuildConfig, RenderConfig};

pub type CompileDiagnostics = HashMap<FileId, (EcoVec<SourceDiagnostic>, EcoVec<SourceDiagnostic>)>;
pub type ProcessDiagnostics = HashMap<FileId, EcoVec<SourceDiagnostic>>;

/// Compiles Typst source files into nodes and maintains the in-memory node
/// store and per-file diagnostics across incremental rebuilds.
#[derive(Default)]
pub struct Compiler<B = String, M = String> {
    file_to_nodes: HashMap<FileId, Vec<NodeId>>,
    node_to_file: HashMap<NodeId, FileId>,
    nodes: HashMap<NodeId, NodeEntry>,
    backmatters: HashMap<NodeId, Backmatter>,
    rendered_bodies: HashMap<NodeId, B>,
    rendered_backmatters: HashMap<NodeId, M>,
    compile_diagnostics: CompileDiagnostics,
    process_diagnostics: ProcessDiagnostics,
    links: DiGraphMap<NodeId, ()>,
    transclusions: DiGraphMap<NodeId, ()>,
    interner: NodeInterner,
    dirty: HashSet<NodeId>,
    removed: HashSet<NodeId>,
    metadata_dirty: HashSet<NodeId>,
}

impl<B, M> Compiler<B, M> {
    /// Compiles a single source file and splits it into nodes, updating the
    /// node store and diagnostics.
    ///
    /// Typst compile errors and node-splitting errors (e.g. duplicate node IDs)
    /// are stored as diagnostics rather than returned as errors.
    pub fn update<W: World>(&mut self, world: &W, id: FileId) {
        self._update(id, extract::compile(world));
    }

    fn _update(
        &mut self,
        id: FileId,
        compiled: Warned<Result<HashMap<String, NodeOutput>, EcoVec<SourceDiagnostic>>>,
    ) {
        let Warned {
            output: result,
            warnings,
        } = compiled;

        match result {
            Ok(extracted_nodes) => {
                // TODO: Cross-file duplicate identifier check. Currently we
                // accept all nodes from a file or reject all on error. Consider
                // partial acceptance: let valid nodes through and only reject
                // nodes that actually have duplicate identifiers.

                // TODO:
                let nodes: HashMap<NodeId, (NodeEntry, Vec<NodeId>, Vec<NodeId>)> = extracted_nodes
                    .into_iter()
                    .map(
                        |(
                            name,
                            NodeOutput {
                                entry,
                                transclusions,
                                links,
                            },
                        )| {
                            let node_id = self.interner.intern(name);
                            let transclusions = transclusions
                                .iter()
                                .map(|t| self.interner.intern(t.as_str()))
                                .collect();
                            let links = links
                                .iter()
                                .map(|l| self.interner.intern(l.as_str()))
                                .collect();
                            (node_id, (entry, transclusions, links))
                        },
                    )
                    .collect();

                let mut metadata_dirty_from_update = HashSet::new();
                // Compare new title/metadata against old entries before
                // remove() clears them, so we can track which nodes had their
                // displayable backmatter information change, adding them to
                // `metadata_dirty`.
                //
                // New nodes are not added to `metadata_dirty`: no existing
                // backmatter can reference a node that didn't exist.
                for (&node_id, (entry, _, _)) in &nodes {
                    if self.nodes.get(&node_id).is_some_and(|old| {
                        old.title != entry.title || old.node_metadata != entry.node_metadata
                    }) {
                        metadata_dirty_from_update.insert(node_id);
                    }
                }

                // Now safe to orphan the old compilation.
                self.remove(id);
                self.metadata_dirty.extend(metadata_dirty_from_update);

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
                self.backmatters.remove(&old_id);
                self.rendered_bodies.remove(&old_id);
                self.rendered_backmatters.remove(&old_id);
                self.dirty.remove(&old_id);
                self.metadata_dirty.remove(&old_id);
                self.removed.insert(old_id);

                clear_outgoing(&mut self.transclusions, old_id);
                clear_outgoing(&mut self.links, old_id);
            }
        }

        self.compile_diagnostics.remove(&id);
    }

    /// Returns all compile-time diagnostics, keyed by source [`FileId`].
    ///
    /// Each entry is a `(warnings, errors)` pair of [`SourceDiagnostic`] vecs.
    pub fn compile_diagnostics(&self) -> &CompileDiagnostics {
        &self.compile_diagnostics
    }

    /// Returns all structural diagnostics produced by the last [`Compiler::process`] call,
    /// keyed by source [`FileId`].
    ///
    /// These are errors detected across the full node graph (e.g. transclusion
    /// cycles) and are recomputed from scratch on every [`Compiler::process`]
    /// call.
    pub fn process_diagnostics(&self) -> &ProcessDiagnostics {
        &self.process_diagnostics
    }

    /// Returns an [`OutputPlan`] describing the writes and deletes to apply to
    /// the output directory, and clears the dirty and removed sets.
    pub(crate) fn _process<R>(&mut self, renderer: &R) -> anyhow::Result<OutputPlan<R::Node>>
    where
        R: Render<Body = B, Backmatter = M>,
    {
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
        let mut outlinks_accumulator: HashMap<NodeId, HashSet<NodeId>> = HashMap::new();
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
                let any_link_affected = scc.iter().any(|&m| {
                    self.links.neighbors(m).any(|t| {
                        dirty.contains(&t) || metadata_dirty.contains(&t) || removed.contains(&t)
                    })
                });
                if any_affected || any_link_affected {
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
                        .any(|t| body_affected.contains(&t) || removed.contains(&t))
                    || self.links.neighbors(id).any(|t| {
                        dirty.contains(&t) || metadata_dirty.contains(&t) || removed.contains(&t)
                    });
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
                    if self.nodes.contains_key(&id) {
                        let new_backmatter = collect_backmatter(
                            id,
                            &self.links,
                            &self.transclusions,
                            &outlinks_accumulator,
                        );
                        outlinks_accumulator.insert(id, new_backmatter.outlinks.clone());

                        if should_backmatter_render(
                            self.backmatters.get(&id),
                            &new_backmatter,
                            &dirty,
                            &metadata_dirty,
                            &removed,
                        ) {
                            self.backmatters.insert(id, new_backmatter);
                            backmatter_affected.insert(id);
                        }
                    }

                    if body_affected.contains(&id) || backmatter_affected.contains(&id) {
                        render_order.push(id);
                    }
                }
            }
        }
        for &id in self
            .nodes
            .keys()
            .filter(|id| !self.transclusions.contains_node(**id))
        {
            if dirty.contains(&id) {
                body_affected.insert(id);
            }
            if self
                .links
                .neighbors(id)
                .any(|t| dirty.contains(&t) || metadata_dirty.contains(&t) || removed.contains(&t))
            {
                body_affected.insert(id);
            }

            let new_backmatter =
                collect_backmatter(id, &self.links, &self.transclusions, &outlinks_accumulator);

            if should_backmatter_render(
                self.backmatters.get(&id),
                &new_backmatter,
                &dirty,
                &metadata_dirty,
                &removed,
            ) {
                self.backmatters.insert(id, new_backmatter);
                backmatter_affected.insert(id);
            }

            if body_affected.contains(&id) || backmatter_affected.contains(&id) {
                render_order.push(id);
            }
        }

        // Pass 2: render nodes in order (isolated first, then leaves-to-roots).

        for &id in &render_order {
            if body_affected.contains(&id) {
                let input =
                    build_body_input(id, &self.nodes, &self.rendered_bodies, &self.interner);
                let rendered_body = renderer.render_body(input)?;

                self.rendered_bodies.insert(id, rendered_body);
            }
            if backmatter_affected.contains(&id) {
                let backmatter = self
                    .backmatters
                    .get(&id)
                    .expect("bug: renderable node has no backmatter after pass 1");
                let input = build_backmatter_input(id, &self.nodes, backmatter, &self.interner);
                let rendered_backmatter = renderer.render_backmatter(input)?;

                self.rendered_backmatters.insert(id, rendered_backmatter);
            }
        }

        let writes = render_order
            .iter()
            .map(|&id| {
                let identifier = self.interner.name(id).to_owned();
                let input = build_node_input(
                    id,
                    &self.nodes,
                    &self.rendered_bodies,
                    &self.rendered_backmatters,
                    &self.interner,
                );
                let html = renderer.render_node(input)?;

                Ok((identifier, html))
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

impl Compiler {
    /// Returns an [`OutputPlan`] describing the writes and deletes to apply to
    /// the output directory, and clears the dirty and removed sets.
    pub fn process(&mut self, config: &RenderConfig) -> anyhow::Result<OutputPlan> {
        let renderer = JinjaRenderer::new(config);

        self._process(&renderer)
    }
}

pub struct OutputPlan<N = String> {
    pub writes: HashMap<String, N>,
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

#[derive(Copy, Clone, Hash, Eq, PartialEq, Ord, PartialOrd, Debug)]
struct NodeId(u32);

/// Interns node name strings to compact [`NodeId`] handles.
#[derive(Clone, Default)]
struct NodeInterner {
    forward: HashMap<String, NodeId>,
    reverse: Vec<String>,
}

impl NodeInterner {
    /// Returns the [`NodeId`] for `name`, interning it if not already present.
    fn intern<S: Into<String> + AsRef<str>>(&mut self, name: S) -> NodeId {
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
    fn get(&self, name: &str) -> Option<NodeId> {
        self.forward.get(name).copied()
    }

    /// Returns the name string for a [`NodeId`].
    ///
    /// Panics if `id` was not produced by this interner.
    fn name(&self, id: NodeId) -> &str {
        &self.reverse[id.0 as usize]
    }
}

/// Cached backmatter sets for a node, used to determine whether backmatter
/// needs to be re-rendered on the next [`Compiler::process`] call.
#[derive(Clone, Default, PartialEq, Eq)]
struct Backmatter {
    pub contexts: HashSet<NodeId>,
    pub backlinks: HashSet<NodeId>,
    pub outlinks: HashSet<NodeId>,
}

pub(crate) type Metadata = HashMap<String, Vec<String>>;

#[derive(Clone, Debug)]
struct NodeEntry {
    pub body_html: String,
    pub title: String,
    pub title_text: String,
    pub span: Span,
    // TODO: Should we intern metadata strings and output? Is that nuts? Would
    // it cause incorrectness?
    pub node_metadata: Metadata,
    pub transclusion_metadata: HashMap<u32, Metadata>,
    pub link_metadata: HashMap<u32, Metadata>,
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

fn collect_backmatter(
    id: NodeId,
    links: &DiGraphMap<NodeId, ()>,
    transclusions: &DiGraphMap<NodeId, ()>,
    outlinks_accumulator: &HashMap<NodeId, HashSet<NodeId>>,
) -> Backmatter {
    let mut outlinks: HashSet<NodeId> = links.neighbors(id).collect();
    for target in transclusions.neighbors(id) {
        if let Some(target_outlinks) = outlinks_accumulator.get(&target) {
            outlinks.extend(target_outlinks.iter().copied());
        }
    }
    let contexts: HashSet<NodeId> = transclusions
        .neighbors_directed(id, Direction::Incoming)
        .collect();
    let backlinks: HashSet<NodeId> = links.neighbors_directed(id, Direction::Incoming).collect();

    Backmatter {
        contexts,
        backlinks,
        outlinks,
    }
}

fn should_backmatter_render(
    option_old: Option<&Backmatter>,
    new: &Backmatter,
    dirty: &HashSet<NodeId>,
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
                .any(|id| dirty.contains(id) || metadata_dirty.contains(id) || removed.contains(id))
    })
}

fn build_body_input<'a, B>(
    id: NodeId,
    nodes: &'a HashMap<NodeId, NodeEntry>,
    rendered_bodies: &'a HashMap<NodeId, B>,
    interner: &'a NodeInterner,
) -> BodyInput<'a, B> {
    // TODO: Perform the dangling link and transclusion checks here?

    let entry = &nodes[&id];
    let document = Document::from(entry.body_html.as_str());

    let links: HashMap<u32, LinkInput<'a>> = document
        .select("a")
        .iter()
        .filter_map(|element| {
            let href = element.attr("href")?;
            let identifier_str = href.strip_prefix("wb:")?;
            let counter: u32 = element
                .attr("data-counter")
                .expect("bug: link missing data-counter")
                .parse()
                .expect("bug: link has invalid data-counter");

            let target_id = interner
                .get(identifier_str)
                .expect("bug: link identifier not interned");
            let identifier = interner.name(target_id);

            let metadata = entry.link_metadata.get(&counter);
            let resolution = nodes.get(&target_id).map(|target| ResolvedLink {
                title: target.title.as_str(),
                title_text: target.title_text.as_str(),
                metadata: &target.node_metadata,
            });

            Some((
                counter,
                LinkInput {
                    identifier,
                    metadata,
                    resolution,
                },
            ))
        })
        .collect();

    let transclusions: HashMap<u32, TransclusionInput<'a, B>> = document
        .select("wb-transclude")
        .iter()
        .map(|element| {
            let identifier_attr = element
                .attr("identifier")
                .expect("bug: wb-transclude missing identifier");
            let counter: u32 = element
                .attr("counter")
                .expect("bug: wb-transclude missing counter")
                .parse()
                .expect("bug: wb-transclude has invalid counter");

            let target_id = interner
                .get(identifier_attr.as_ref())
                .expect("bug: transclusion identifier not interned");
            let identifier = interner.name(target_id);

            let metadata = entry.transclusion_metadata.get(&counter);
            let resolution = nodes.get(&target_id).map(|target| {
                let body = rendered_bodies
                    .get(&target_id)
                    .expect("bug: transclusion target has no rendered_body");
                ResolvedTransclusion {
                    identifier,
                    title: target.title.as_str(),
                    title_text: target.title_text.as_str(),
                    metadata: &target.node_metadata,
                    body,
                }
            });

            (
                counter,
                TransclusionInput {
                    metadata,
                    resolution,
                },
            )
        })
        .collect();

    BodyInput {
        body_html: entry.body_html.as_str(),
        links,
        transclusions,
    }
}

fn build_backmatter_input<'a>(
    id: NodeId,
    nodes: &'a HashMap<NodeId, NodeEntry>,
    backmatter: &'a Backmatter,
    interner: &'a NodeInterner,
) -> BackmatterInput<'a> {
    let entry = &nodes[&id];
    let node = (
        interner.name(id).to_owned(),
        BackmatterNode {
            title: entry.title.as_str(),
            title_text: entry.title_text.as_str(),
            metadata: &entry.node_metadata,
        },
    );

    let backmatter_set = |ids: &HashSet<NodeId>| -> Vec<(String, Option<BackmatterNode<'a>>)> {
        let mut items: Vec<(String, Option<BackmatterNode<'a>>)> = ids
            .iter()
            .map(|&nid| {
                let name = interner.name(nid).to_owned();
                let node = nodes.get(&nid).map(|target| BackmatterNode {
                    title: target.title.as_str(),
                    title_text: target.title_text.as_str(),
                    metadata: &target.node_metadata,
                });
                (name, node)
            })
            .collect();
        items.sort_by(|a, b| a.0.cmp(&b.0));

        items
    };

    BackmatterInput {
        node,
        contexts: backmatter_set(&backmatter.contexts),
        backlinks: backmatter_set(&backmatter.backlinks),
        outlinks: backmatter_set(&backmatter.outlinks),
    }
}

fn build_node_input<'a, B, M>(
    id: NodeId,
    nodes: &'a HashMap<NodeId, NodeEntry>,
    rendered_bodies: &'a HashMap<NodeId, B>,
    rendered_backmatters: &'a HashMap<NodeId, M>,
    interner: &'a NodeInterner,
) -> NodeInput<'a, B, M> {
    let entry = &nodes[&id];
    let body = rendered_bodies
        .get(&id)
        .expect("bug: renderable node has no rendered_body after pass 2");
    let backmatter = rendered_backmatters
        .get(&id)
        .expect("bug: renderable node has no rendered backmatter after pass 2");

    NodeInput {
        identifier: interner.name(id).to_owned(),
        title: entry.title.as_str(),
        title_text: entry.title_text.as_str(),
        metadata: &entry.node_metadata,
        body,
        backmatter,
    }
}
