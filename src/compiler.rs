mod extract;
mod render;
#[cfg(test)]
mod tests;

use std::collections::hash_map::Entry;
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
pub struct Compiler<T = String, U = String> {
    file_to_nodes: HashMap<FileId, HashSet<NodeId>>,
    nodes: HashMap<NodeId, EcoVec<NodeEntry<NodeId>>>,
    backmatters: HashMap<NodeId, Backmatter>,
    rendered_bodies: HashMap<NodeId, T>,
    rendered_backmatters: HashMap<NodeId, U>,
    compile_diagnostics: CompileDiagnostics,
    process_diagnostics: ProcessDiagnostics,
    interner: NodeInterner,
    dirty: HashSet<NodeId>,
    removed: HashSet<NodeId>,
    outputs: HashSet<NodeId>,
}

impl<T, U> Compiler<T, U> {
    pub fn update<W: World>(&mut self, world: &W, file_id: FileId) {
        self._update(file_id, extract::compile(world));
    }

    fn _update(
        &mut self,
        file_id: FileId,
        compiled: Warned<Result<Vec<NodeOutput>, EcoVec<SourceDiagnostic>>>,
    ) {
        let Warned {
            output: result,
            warnings,
        } = compiled;

        self.remove(file_id);

        match result {
            Ok(outputs) => {
                let mut node_ids = HashSet::new();

                for output in outputs {
                    let node_id = self.interner.intern(output.identifier);
                    let entry = NodeEntry {
                        node: output.node,
                        file_id,
                        transclusions: output
                            .transclusions
                            .into_iter()
                            .map(|s| self.interner.intern(s))
                            .collect(),
                        links: output
                            .links
                            .into_iter()
                            .map(|s| self.interner.intern(s))
                            .collect(),
                    };

                    node_ids.insert(node_id);
                    self.nodes.entry(node_id).or_default().push(entry);

                    self.dirty.insert(node_id);
                    self.removed.remove(&node_id);
                }

                self.file_to_nodes.insert(file_id, node_ids);

                if warnings.is_empty() {
                    self.compile_diagnostics.remove(&file_id);
                } else {
                    self.compile_diagnostics
                        .insert(file_id, (warnings, EcoVec::new()));
                }
            }
            Err(errors) => {
                self.compile_diagnostics.insert(file_id, (warnings, errors));
            }
        }
    }

    pub fn remove(&mut self, file_id: FileId) {
        if let Some(node_ids) = self.file_to_nodes.remove(&file_id) {
            for node_id in node_ids {
                let still_present = {
                    let mut entry = match self.nodes.entry(node_id) {
                        Entry::Occupied(entry) => entry,
                        Entry::Vacant(_) => {
                            panic!("bug: `files_to_nodes` contains a node id not in `nodes`")
                        }
                    };
                    let entries = entry.get_mut();

                    entries.retain(|e| e.file_id != file_id);

                    if entries.is_empty() {
                        entry.remove();
                        false
                    } else {
                        true
                    }
                };

                if still_present {
                    self.dirty.insert(node_id);
                } else {
                    self.dirty.remove(&node_id);
                    self.removed.insert(node_id);
                }
            }
        }

        self.compile_diagnostics.remove(&file_id);
    }

    pub fn compile_diagnostics(&self) -> &CompileDiagnostics {
        &self.compile_diagnostics
    }

    pub fn process_diagnostics(&self) -> &ProcessDiagnostics {
        &self.process_diagnostics
    }

    pub fn _process<R>(&mut self, renderer: &R) -> anyhow::Result<OutputPlan<R::Node>>
    where
        R: Render<Body = T, Backmatter = U>,
    {
        assert!(self.dirty.is_disjoint(&self.removed));

        if self.dirty.is_empty() && self.removed.is_empty() {
            return Ok(OutputPlan {
                writes: HashMap::new(),
                deletes: HashSet::new(),
            });
        }

        let (render_plan, deletes) = self.process_stage1();
        let writes = self.process_stage2(renderer, &render_plan)?;

        Ok(OutputPlan { writes, deletes })
    }

    fn process_stage1(&mut self) -> (RenderPlan, HashSet<String>) {
        self.process_diagnostics.clear();
        let process_diagnostics: &mut ProcessDiagnostics = &mut self.process_diagnostics;

        let dirty = std::mem::take(&mut self.dirty);
        let removed = std::mem::take(&mut self.removed);

        let backmatters: &mut HashMap<NodeId, Backmatter> = &mut self.backmatters;

        let nodes: &HashMap<NodeId, EcoVec<NodeEntry<NodeId>>> = &self.nodes;
        let interner: &NodeInterner = &self.interner;
        let previous_outputs: &HashSet<NodeId> = &self.outputs;

        // Build the transition and link graphs. At the same time, partition
        // `nodes` into singletons and duplicates, emitting diagnostics for
        // duplicates.

        let mut transclusions: DiGraphMap<NodeId, ()> = DiGraphMap::new();
        let mut links: DiGraphMap<NodeId, ()> = DiGraphMap::new();

        for (&node_id, entries) in nodes {
            assert!(!entries.is_empty(), "All entries in `nodes` are non-empty");

            if entries.len() == 1 {
                let entry = &entries[0];
                for &target in &entry.transclusions {
                    transclusions.add_edge(node_id, target, ());
                }
                for &target in &entry.links {
                    links.add_edge(node_id, target, ());
                }
            } else {
                let name = interner.name(node_id);
                for entry in entries {
                    process_diagnostics
                        .entry(entry.file_id)
                        .or_default()
                        .push(duplicate_node_identifier_diagnostic(name));
                }
            }
        }

        // Emit warning diagnostics for dangling transclusions and links.

        let is_singleton = |node_id| {
            nodes
                .get(&node_id)
                .is_some_and(|entries| entries.len() == 1)
        };

        for (source, destination, _) in transclusions
            .all_edges()
            .filter(|&(_, destination, _)| is_singleton(destination))
        {
            let file_id = nodes[&source][0].file_id;
            let name = interner.name(destination);
            process_diagnostics
                .entry(file_id)
                .or_default()
                .push(dangling_transclusion_diagnostic(name));
        }
        for (source, destination, _) in links
            .all_edges()
            .filter(|&(_, destination, _)| is_singleton(destination))
        {
            let file_id = nodes[&source][0].file_id;
            let name = interner.name(destination);
            process_diagnostics
                .entry(file_id)
                .or_default()
                .push(dangling_link_diagnostic(name));
        }

        let mut body_affected = HashSet::new();
        let mut backmatter_affected = HashSet::new();
        let mut unrenderable = HashSet::new();
        let mut render_plan = Vec::new();

        // Invariant:
        // By the time a renderable node id is processed, every renderable transclusion target has an up-to-date entry in self.backmatters.

        let sccs = tarjan_scc(&transclusions);

        for scc in &sccs {
            let node_id = scc[0];
            let is_cyclic = scc.len() > 1 || transclusions.contains_edge(node_id, node_id);

            if is_cyclic {
            } else {
                // The rendered output of `render_body` can change if this node
                // is dirty, if any nodes it transcludes are dirty or removed,
                // or if any if its outgoing links are dirty or removed (since
                // the target node's title or metadata could have changed). This
                // is more conservative than we would like. Non-title or
                // -metadata changes to a link target should not trigger a
                // rerender, but we don't currently have a mechanism for this.
                let is_body_affected = dirty.contains(&node_id)
                    || transclusions
                        .neighbors(node_id)
                        .any(|t| body_affected.contains(&t) || removed.contains(&t))
                    || links
                        .neighbors(node_id)
                        .any(|l| dirty.contains(&l) || removed.contains(&l));
                if is_body_affected {
                    body_affected.insert(node_id);
                }

                if transclusions
                    .neighbors(node_id)
                    .any(|t| unrenderable.contains(&node_id))
                {
                    unrenderable.insert(node_id);
                } else if is_singleton(node_id) {
                    let new_backmatter =
                        collect_backmatter(node_id, &links, &transclusions, &backmatters);
                    let old_backmatter = backmatters.insert(node_id, new_backmatter);

                    if should_backmatter_render(
                        old_backmatter.as_ref(),
                        &new_backmatter,
                        &dirty,
                        &removed,
                    ) {
                        backmatter_affected.insert(node_id);
                    }

                    let is_body_affected = body_affected.contains(&node_id);
                    let is_backmatter_affected = backmatter_affected.contains(&node_id);

                    if is_body_affected || is_backmatter_affected {
                        render_plan.push(RenderItem {
                            node_id,
                            needs_body: is_body_affected,
                            needs_backmatter: is_backmatter_affected,
                        });
                    }
                }
            }
        }

        todo!()
    }

    fn process_stage2<R>(
        &mut self,
        renderer: &R,
        render_plan: &RenderPlan,
    ) -> anyhow::Result<HashMap<String, R::Node>>
    where
        R: Render<Body = T, Backmatter = U>,
    {
        let nodes: &HashMap<_, _> = &self.nodes;
        let backmatters: &HashMap<NodeId, Backmatter> = &self.backmatters;
        let rendered_bodies: &mut HashMap<NodeId, R::Body> = &mut self.rendered_bodies;
        let rendered_backmatters: &mut HashMap<NodeId, R::Backmatter> =
            &mut self.rendered_backmatters;
        let interner: &NodeInterner = &self.interner;

        let mut writes = HashMap::with_capacity(render_plan.len());

        for &RenderItem {
            node_id,
            needs_backmatter,
            needs_body,
        } in render_plan
        {
            assert!(
                needs_body || needs_backmatter,
                "One of `needs_body` or `needs_backmatter` holds"
            );

            if needs_body {
                let rendered_body = renderer.render_body(body_input(
                    |node_id| nodes_helper(nodes, node_id),
                    rendered_bodies,
                    interner,
                    node_id,
                ))?;

                rendered_bodies.insert(node_id, rendered_body);
            }

            if needs_backmatter {
                let backmatter = backmatters
                    .get(&node_id)
                    .expect("bug: renderable node has no backmatter after stage 1");
                let rendered_backmatter = renderer.render_backmatter(backmatter_input(
                    |node_id| nodes_helper(nodes, node_id),
                    backmatter,
                    interner,
                ))?;

                rendered_backmatters.insert(node_id, rendered_backmatter);
            }

            let html = renderer.render_node(node_input(
                |node_id| nodes_helper(nodes, node_id),
                rendered_bodies,
                rendered_backmatters,
                interner,
                node_id,
            ))?;
            let previous = writes.insert(interner.name(node_id).to_owned(), html);
            assert!(
                previous.is_none(),
                "Render plan does not contain duplicates"
            );
        }

        return Ok(writes);

        fn nodes_helper(
            nodes: &HashMap<NodeId, EcoVec<NodeEntry<NodeId>>>,
            node_id: NodeId,
        ) -> Option<&Node> {
            nodes
                .get(&node_id)
                .filter(|entries| entries.len() == 1)
                .map(|entries| &entries[0].node)
        }
    }
}

impl Compiler {
    pub fn process(&mut self, config: &RenderConfig) -> anyhow::Result<OutputPlan> {
        let renderer = JinjaRenderer::new(config);

        self._process(&renderer)
    }
}

impl<T, U> Default for Compiler<T, U> {
    fn default() -> Self {
        Self {
            file_to_nodes: HashMap::default(),
            nodes: HashMap::default(),
            backmatters: HashMap::default(),
            rendered_bodies: HashMap::default(),
            rendered_backmatters: HashMap::default(),
            compile_diagnostics: HashMap::default(),
            process_diagnostics: HashMap::default(),
            interner: NodeInterner::default(),
            outputs: HashSet::default(),
            dirty: HashSet::default(),
            removed: HashSet::default(),
        }
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

// TODO: Make EcoVec
pub(crate) type Metadata = HashMap<String, Vec<String>>;

#[derive(Clone, Debug)]
struct NodeEntry<T> {
    pub node: Node,
    pub file_id: FileId,
    pub links: EcoVec<T>,
    pub transclusions: EcoVec<T>,
}

#[derive(Clone, Debug)]
struct Node {
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

struct RenderItem {
    node_id: NodeId,
    needs_backmatter: bool,
    needs_body: bool,
}

type RenderPlan = Vec<RenderItem>;

// TODO: Future improvement: We have spans, we should not use detached for these diagnostics

fn duplicate_node_identifier_diagnostic(name: &str) -> SourceDiagnostic {
    SourceDiagnostic::error(
        Span::detached(),
        eco_format!("duplicate node identifier across files: {name:?}"),
    )
}

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
    let (files, mut names): (HashSet<FileId>, Vec<&str>) = pairs.unzip();
    names.sort();
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
    node_id: NodeId,
    links: &DiGraphMap<NodeId, ()>,
    transclusions: &DiGraphMap<NodeId, ()>,
    backmatters: &HashMap<NodeId, Backmatter>,
) -> Backmatter {
    let mut outlinks: HashSet<NodeId> = links.neighbors(node_id).collect();
    for target in transclusions.neighbors(node_id) {
        if let Some(target_outlinks) = backmatters
            .get(&target)
            .map(|backmatter| &backmatter.outlinks)
        {
            outlinks.extend(target_outlinks.iter().copied());
        }
    }
    let contexts: HashSet<NodeId> = transclusions
        .neighbors_directed(node_id, Direction::Incoming)
        .collect();
    let backlinks: HashSet<NodeId> = links
        .neighbors_directed(node_id, Direction::Incoming)
        .collect();

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
    removed: &HashSet<NodeId>,
) -> bool {
    option_old.is_none_or(|old| {
        old != new
            || new
                .contexts
                .iter()
                .chain(new.backlinks.iter())
                .chain(new.outlinks.iter())
                .any(|id| dirty.contains(id) || removed.contains(id))
    })
}

fn body_input<'a, B>(
    nodes: impl Fn(NodeId) -> Option<&'a Node>,
    rendered_bodies: &'a HashMap<NodeId, B>,
    interner: &'a NodeInterner,
    node_id: NodeId,
) -> BodyInput<'a, B> {
    let node = nodes(node_id).expect("bug: node in render plan does not exist");

    let document = Document::from(node.body_html.as_str());

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

            let metadata = node.link_metadata.get(&counter);
            let resolution = nodes(target_id).map(|target| ResolvedLink {
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

            let metadata = node.transclusion_metadata.get(&counter);
            let resolution = nodes(target_id).map(|target| {
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
        body_html: node.body_html.as_str(),
        links,
        transclusions,
    }
}

fn backmatter_input<'a>(
    nodes: impl Fn(NodeId) -> Option<&'a Node>,
    backmatter: &'a Backmatter,
    interner: &'a NodeInterner,
) -> BackmatterInput<'a> {
    let backmatter_set = |ids: &HashSet<NodeId>| -> Vec<(String, Option<BackmatterNode<'a>>)> {
        let mut items: Vec<(String, Option<BackmatterNode<'a>>)> = ids
            .iter()
            .map(|&node_id| {
                let name = interner.name(node_id).to_owned();
                let node = nodes(node_id).map(|target| BackmatterNode {
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
        contexts: backmatter_set(&backmatter.contexts),
        backlinks: backmatter_set(&backmatter.backlinks),
        outlinks: backmatter_set(&backmatter.outlinks),
    }
}

fn node_input<'a, B, M>(
    nodes: impl Fn(NodeId) -> Option<&'a Node>,
    rendered_bodies: &'a HashMap<NodeId, B>,
    rendered_backmatters: &'a HashMap<NodeId, M>,
    interner: &'a NodeInterner,
    node_id: NodeId,
) -> NodeInput<'a, B, M> {
    let node = nodes(node_id).expect("bug: node in render plan does not exist");
    let body = rendered_bodies
        .get(&node_id)
        .expect("bug: renderable node has no rendered_body after pass 2");
    let backmatter = rendered_backmatters
        .get(&node_id)
        .expect("bug: renderable node has no rendered backmatter after pass 2");

    NodeInput {
        identifier: interner.name(node_id).to_owned(),
        title: node.title.as_str(),
        title_text: node.title_text.as_str(),
        metadata: &node.node_metadata,
        body,
        backmatter,
    }
}
