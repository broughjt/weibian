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
    // node_to_file: HashMap<NodeId, FileId>,
    nodes: HashMap<NodeId, EcoVec<Node<NodeId>>>,
    backmatters: HashMap<NodeId, Backmatter>,
    rendered_bodies: HashMap<NodeId, T>,
    rendered_backmatters: HashMap<NodeId, U>,
    compile_diagnostics: CompileDiagnostics,
    process_diagnostics: ProcessDiagnostics,
    // TODO: remove
    // links: DiGraphMap<NodeId, ()>,
    // transclusions: DiGraphMap<NodeId, ()>,
    interner: NodeInterner,
    dirty: HashSet<NodeId>,
    removed: HashSet<NodeId>,
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

                for (identifier, node) in outputs {
                    let node_id = self.interner.intern(identifier);
                    let node = Node {
                        body_html: node.body_html,
                        title: node.title,
                        title_text: node.title_text,
                        file_id,
                        span: node.span,
                        node_metadata: node.node_metadata,
                        transclusions: node
                            .transclusions
                            .into_iter()
                            .map(|s| self.interner.intern(s))
                            .collect(),
                        transclusion_metadata: node.transclusion_metadata,
                        links: node
                            .links
                            .into_iter()
                            .map(|s| self.interner.intern(s))
                            .collect(),
                        link_metadata: node.link_metadata,
                    };

                    node_ids.insert(node_id);
                    self.nodes.entry(node_id).or_default().push(node);

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
                {
                    let mut entry = match self.nodes.entry(node_id) {
                        Entry::Occupied(entry) => entry,
                        Entry::Vacant(_) => {
                            panic!("bug: `files_to_nodes` contains a node id not in `nodes`")
                        }
                    };
                    let nodes = entry.get_mut();

                    nodes.retain(|n| n.file_id != file_id);

                    if nodes.is_empty() {
                        entry.remove();
                    }
                }

                self.backmatters.remove(&node_id);
                self.rendered_bodies.remove(&node_id);
                self.rendered_backmatters.remove(&node_id);

                self.dirty.remove(&node_id);
                self.removed.insert(node_id);
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
        // TODO: Pattern match on self
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
                let rendered_body =
                    renderer.render_body(body_input(nodes, rendered_bodies, interner, node_id))?;

                rendered_bodies.insert(node_id, rendered_body);
            }

            if needs_backmatter {
                let backmatter = backmatters
                    .get(&node_id)
                    .expect("bug: renderable node has no backmatter after stage 1");
                let rendered_backmatter =
                    renderer.render_backmatter(backmatter_input(nodes, backmatter, interner))?;

                rendered_backmatters.insert(node_id, rendered_backmatter);
            }

            let html = renderer.render_node(node_input(
                nodes,
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

        Ok(writes)
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
struct Node<T> {
    pub body_html: String,
    pub title: String,
    pub title_text: String,
    pub file_id: FileId,
    pub span: Span,
    // TODO: Should we intern metadata strings and output? Is that nuts? Would
    // it cause incorrectness?
    pub node_metadata: Metadata,
    pub transclusions: EcoVec<T>,
    pub transclusion_metadata: HashMap<u32, Metadata>,
    pub links: EcoVec<T>,
    pub link_metadata: HashMap<u32, Metadata>,
}

struct RenderItem {
    node_id: NodeId,
    needs_backmatter: bool,
    needs_body: bool,
}

type RenderPlan = Vec<RenderItem>;

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

fn body_input<'a, B>(
    nodes: &'a HashMap<NodeId, EcoVec<Node<NodeId>>>,
    rendered_bodies: &'a HashMap<NodeId, B>,
    interner: &'a NodeInterner,
    node_id: NodeId,
) -> BodyInput<'a, B> {
    let node = {
        let entry = &nodes[&node_id];
        assert!(
            entry.len() == 1,
            "Node in render plan exists and is not duplicated"
        );

        &entry[0]
    };

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
            let resolution = nodes.get(&target_id).map(|nodes| {
                assert!(
                    nodes.len() == 1,
                    "Node entry in render plan which exists is non-empty and is not duplicated"
                );
                let target = &nodes[0];

                ResolvedLink {
                    title: target.title.as_str(),
                    title_text: target.title_text.as_str(),
                    metadata: &target.node_metadata,
                }
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
            let resolution = nodes.get(&target_id).map(|nodes| {
                assert!(
                    nodes.len() == 1,
                    "Node entry in render plan which exists is non-empty and is not duplicated"
                );
                let target = &nodes[0];

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
    nodes: &'a HashMap<NodeId, EcoVec<Node<NodeId>>>,
    backmatter: &'a Backmatter,
    interner: &'a NodeInterner,
) -> BackmatterInput<'a> {
    let backmatter_set = |ids: &HashSet<NodeId>| -> Vec<(String, Option<BackmatterNode<'a>>)> {
        let mut items: Vec<(String, Option<BackmatterNode<'a>>)> = ids
            .iter()
            .map(|&nid| {
                let name = interner.name(nid).to_owned();
                let node = nodes.get(&nid).map(|nodes| {
                    assert!(
                        nodes.len() == 1,
                        "Node entry in render plan which exists is non-empty and is not duplicated"
                    );
                    let target = &nodes[0];

                    BackmatterNode {
                        title: target.title.as_str(),
                        title_text: target.title_text.as_str(),
                        metadata: &target.node_metadata,
                    }
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
    nodes: &'a HashMap<NodeId, EcoVec<Node<NodeId>>>,
    rendered_bodies: &'a HashMap<NodeId, B>,
    rendered_backmatters: &'a HashMap<NodeId, M>,
    interner: &'a NodeInterner,
    node_id: NodeId,
) -> NodeInput<'a, B, M> {
    let node = {
        let entry = &nodes[&node_id];
        assert!(
            entry.len() == 1,
            "Node in render plan exists and is not duplicated"
        );

        &entry[0]
    };
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
