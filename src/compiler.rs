use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;

use crate::config::{BuildConfig, NODE_TEMPLATE};
use dom_query::Document;
use ecow::{EcoVec, eco_format};
use petgraph::algo::tarjan_scc;
use petgraph::graphmap::DiGraphMap;
use petgraph::visit::{Bfs, Reversed};
use typst::World;
use typst::diag::{Severity, SourceDiagnostic, Warned};
use typst::syntax::{FileId, Span};
use typst_html::{HtmlAttr, HtmlDocument, HtmlNode, HtmlTag};

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
}

impl Compiler {
    /// Compiles a single source file and splits it into nodes, updating the
    /// node store and diagnostics.
    ///
    /// Typst compile errors and node-splitting errors (e.g. duplicate node IDs)
    /// are stored as diagnostics rather than returned as errors.
    pub fn compile<W: World>(&mut self, world: &W, id: FileId) {
        // Orphan all nodes from the previous compilation; nodes that reappear
        // are de-orphaned in the `Ok` branch below.
        self.remove(id);

        let Warned {
            output: result,
            mut warnings,
        } = typst::compile::<HtmlDocument>(world);

        // Discard warnings about html being an unstable feature, html is kind
        // of the whole game here
        warnings.retain(|diagnostic: &mut SourceDiagnostic| {
            !(diagnostic.severity == Severity::Warning && diagnostic.message == HTML_MESSAGE)
        });

        let nodes_result = result.and_then(|html_document| {
            typst_html::html(&html_document).and_then(|content| {
                let document = Document::from(content);

                extract(&html_document, document, &mut self.interner, |id| {
                    self.nodes.contains_key(&id)
                })
            })
        });

        match nodes_result {
            Ok(nodes) => {
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
    pub fn compile_diagnostics(
        &self,
    ) -> &HashMap<FileId, (EcoVec<SourceDiagnostic>, EcoVec<SourceDiagnostic>)> {
        &self.compile_diagnostics
    }

    /// Returns all structural diagnostics produced by the last [`process`] call,
    /// keyed by source [`FileId`].
    ///
    /// These are errors detected across the full node graph (e.g. transclusion
    /// cycles) and are recomputed from scratch on every [`process`] call.
    pub fn process_diagnostics(&self) -> &HashMap<FileId, EcoVec<SourceDiagnostic>> {
        &self.process_diagnostics
    }

    /// Returns an [`OutputPlan`] describing the writes and deletes to apply to
    /// the output directory, and clears the dirty and removed sets.
    pub fn process(&mut self, config: &BuildConfig) -> anyhow::Result<OutputPlan> {
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
            let file_id = self
                .node_to_file
                .get(&source)
                .copied()
                .expect("bug: node in transclusion graph has no file entry");
            let name = self.interner.name(destination);

            self.process_diagnostics
                .entry(file_id)
                .or_default()
                .push(SourceDiagnostic::warning(
                    Span::detached(),
                    eco_format!("dangling transclusion: {name}"),
                ));
        }

        // Warn on dangling links (target node does not exist).
        for (source, destination, _) in self
            .links
            .all_edges()
            .filter(|&(_, destination, _)| !self.nodes.contains_key(&destination))
        {
            let file_id = self
                .node_to_file
                .get(&source)
                .copied()
                .expect("bug: node in link graph has no file entry");
            let name = self.interner.name(destination);

            self.process_diagnostics
                .entry(file_id)
                .or_default()
                .push(SourceDiagnostic::warning(
                    Span::detached(),
                    eco_format!("dangling link: {name}"),
                ));
        }

        // Compute the render set
        //
        // Consider the reverse transclusion graph. In the normal transclusion
        // graph, an edge $i \to j$ means $i$ transcludes $j$; the reverse graph
        // flips this so that $i \to j$ means $i$ is transcluded by $j$.
        //
        // We want to (re)render the set of nodes reachable from the dirty set
        // in the reverse transclusion graph, since part of their content has
        // been invalidated. For the same reason, we want to render nodes
        // reachable from the removed set, but not the members of the removed
        // set themselves, since we should remove those not render them.
        //
        // Formally, let $D$ be the dirty set and $R$ be the removed set. Then
        // let $D^{*}$ be the set of nodes reachable from $D$ in the reverse
        // transclusion graph, and let $R^{*}$ be the set of nodes reachable
        // from $R$ in the reverse transclusion graph. We want to render the
        // set
        //
        // $D^{*} \cup (R^{*} \setminus R)$.
        //
        // We note that [`Compiler::remove`] and [`Compiler::compile`] maintain
        // the invariant that the dirty set and the removed set are disjoint,
        // that is, $D \cap R = \emptyset$. Moreover, since [`Compiler::remove`]
        // clears outgoing edges from removed nodes in the transclusion graph,
        // elements of $R$ have no incoming edges in the reverse transclusion
        // graph---BFS from $D$ cannot reach them---giving $D^{*} \cap R =
        // \emptyset$. Using this, we can write
        //
        // $D^{*} \cup (R^{*} \setminus R) =
        //  (D^{*} \setminus R) \cup (R^{*} \setminus R) =
        //  (D^{*} \cup R^{*}) \setminus R.
        //
        // We compute the right-hand side below as `render`. We insert nodes
        // reachable from $D$ or $R$, and remove each member of $R$ in the
        // second for loop.

        assert!(self.dirty.is_disjoint(&self.removed));

        let dirty = std::mem::take(&mut self.dirty);
        let removed = std::mem::take(&mut self.removed);
        let reversed = Reversed(&self.transclusions);
        let mut render: HashSet<NodeId> = HashSet::new();

        for &start in &dirty {
            let mut bfs = Bfs::new(reversed, start);
            while let Some(id) = bfs.next(reversed) {
                render.insert(id);
            }
        }
        for &start in &removed {
            let mut bfs = Bfs::new(reversed, start);
            while let Some(id) = bfs.next(reversed) {
                render.insert(id);
            }
            render.remove(&start);
        }

        // Pass 1: Detect cycles, compute the unrenderable set, compute
        // rendering order

        // The unrenderable set consists of all nodes in the transclusion graph
        // which lie in a cycle and all nodes which directly or transitively
        // transclude a cyclic node. We use Tarjan's algorithm to compute
        // strongly connected components (SCCs). A strongly connected component
        // is cyclic if it has length greater than one or if its only node has a
        // self-loop.

        // We can compute the unrenderable set in one pass, since `tarjan_scc`
        // returns SCCs in reverse topological order (leaves first). When we
        // visit a node $v$, every node that $v$ directly transcluded has
        // already been visited. If any such neighbor is unrenderable---whether
        // because it lies in a cycle or because it in turn depends on an
        // unrenderable node---it is already in the unrenderable set. Therefore,
        // if $v$ does not lie in a cycle, it suffices to check whether any of
        // its neighbors lies in the unrenderable set.

        // To compute the rendering order, we note that isolated nodes can be
        // rendered in any order, so we initialize `render_order` with them
        // before the first pass. Every non-isolated node in the render set is
        // in the transclusion graph---either as a dirty node with transclusion
        // edges, or as a node reached by BFS through the graph. Since
        // `tarjan_scc` covers all nodes in the transclusion graph, the SCC loop
        // accounts for every non-isolated node in the render set. Renderable
        // nodes in the render set are appended to `render_order` upon
        // visitation, so the reverse topological ordering of the SCCs is
        // inherited by `render_order`.
        let mut render_order: Vec<NodeId> = dirty
            .iter()
            .filter(|&&id| !self.transclusions.contains_node(id))
            .copied()
            .collect();
        let sccs = tarjan_scc(&self.transclusions);
        let mut unrenderable: HashSet<NodeId> = HashSet::new();
        for scc in &sccs {
            let id = scc[0];
            let is_cyclic = scc.len() > 1 || self.transclusions.contains_edge(id, id);

            if is_cyclic {
                let names: Vec<&str> = scc.iter().map(|&id| self.interner.name(id)).collect();
                let message = eco_format!("transclusion cycle: {}", names.join(", "));

                unrenderable.extend(scc.iter());

                let files_in_cycle: HashSet<FileId> = scc
                    .iter()
                    .map(|&id| {
                        *self
                            .node_to_file
                            .get(&id)
                            .expect("bug: node in transclusion graph has no file entry")
                    })
                    .collect();

                for file_id in files_in_cycle {
                    // TODO: Store spans in the compiler state so we can
                    // point out the file locations of the offending
                    // transclusions
                    self.process_diagnostics
                        .entry(file_id)
                        .or_default()
                        .push(SourceDiagnostic::error(Span::detached(), message.clone()));
                }
            } else if self
                .transclusions
                .neighbors(id)
                .any(|neighbor| unrenderable.contains(&neighbor))
            {
                unrenderable.insert(id);
            } else if !unrenderable.contains(&id) && render.contains(&id) {
                render_order.push(id);
            }
        }

        // Pass 2: render nodes in order (isolated first, then leaves-to-roots).

        for &id in &render_order {
            let raw_html = self.nodes[&id].raw_html.as_str();
            let document = Document::from(raw_html);

            // Note: if the node has transclusions, which we substitute below,
            // they have already had their anchors properly rendered. We do this
            // before transclusion substitution to avoid doing unnecessary work.
            for element in document.select("a").iter() {
                if let Some(href) = element.attr("href")
                    && let Some(node_id) = href.strip_prefix("wb:")
                {
                    // TODO: Support configurable index node, root
                    // directory, and trailing slash.
                    element.set_attr("href", &format!("/{node_id}.html"));
                }
            }

            // Checking whether the node has any neighbors in the transclusion
            // graph is faster than walking the HTML for `wb-transclude`
            // elements and finding none.
            if self.transclusions.neighbors(id).next().is_some() {
                for element in document.select("wb-transclude").iter() {
                    let identifier = element
                        .attr("identifier")
                        .expect("bug: wb-transclude is missing an identifier");
                    let target_id = self
                        .interner
                        .get(identifier.as_ref())
                        .expect("bug: wb-transclude identifier was not interned");

                    if let Some(entry) = self.nodes.get(&target_id) {
                        let body = entry
                            .rendered_body
                            .as_deref()
                            .expect("bug: wb-transclude target has no rendered_body");
                        let replacement = format!(
                            "<section class=\"block\"><details open><summary><header><h1>{}</h1></header></summary>{body}</details></section>",
                            entry.title
                        );

                        element.replace_with_html(replacement);
                    } else {
                        element.remove();
                    }
                }
            }

            let rendered_body = document.select("body").first().inner_html().to_string();

            self.nodes.get_mut(&id).unwrap().rendered_body = Some(rendered_body);
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
                let template = config
                    .environment
                    .get_template(NODE_TEMPLATE)
                    .expect("bug: node.html template missing from environment");
                let html = template
                    .render(minijinja::context! {
                        node => minijinja::context! {
                            id => name,
                            title => entry.title.as_str(),
                            title_text => entry.title_text.as_str(),
                            body => body,
                        }
                    })
                    .map_err(|e| {
                        anyhow::anyhow!("failed to render template for node {name}: {e}")
                    })?;

                Ok((name.to_owned(), html))
            })
            .collect::<anyhow::Result<_>>()?;
        let deletes = removed
            .iter()
            .chain(unrenderable.intersection(&render))
            .map(|&id| self.interner.name(id).to_string())
            .collect();

        Ok(OutputPlan { writes, deletes })
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

#[derive(Default)]
pub struct NodeEntry {
    pub raw_html: String,
    pub title: String,
    pub title_text: String,
    pub rendered_body: Option<String>,
    pub rendered_backmatter: Option<String>,
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

pub struct OutputPlan {
    pub writes: HashMap<String, String>,
    pub deletes: HashSet<String>,
}

impl OutputPlan {
    pub fn apply(&self, output_directory: &Path) -> Result<(), io::Error> {
        for (node_id, html) in &self.writes {
            std::fs::write(output_directory.join(format!("{node_id}.html")), html)?;
        }
        for node_id in &self.deletes {
            let path = output_directory.join(format!("{node_id}.html"));

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

type ExtractData = (NodeEntry, Vec<NodeId>, Vec<NodeId>);

/// Parses `document` into a map of node IDs to node entries.
///
/// Subnodes are replaced with `<wb-transclude>` references (or removed) as a
/// side effect of extraction. `node_exists` is called to detect cross-file
/// duplicate identifiers.
///
/// Returns `Err` with all collected diagnostics if any validation errors occur,
/// or `Ok` with the node map on success.
fn extract(
    html_document: &HtmlDocument,
    document: Document,
    interner: &mut NodeInterner,
    node_exists: impl Fn(NodeId) -> bool,
) -> Result<HashMap<NodeId, ExtractData>, EcoVec<SourceDiagnostic>> {
    let (spans, mut errors) = collect_node_spans(html_document);

    // Check for global duplicate identifiers before processing.
    errors.extend(
        spans
            .iter()
            .filter(|(id, _)| node_exists(interner.intern(*id)))
            .map(|(id, &span)| {
                SourceDiagnostic::error(span, eco_format!("duplicate node identifier: {id:?}"))
            }),
    );

    let mut nodes: HashMap<NodeId, (NodeEntry, Vec<NodeId>, Vec<NodeId>)> = HashMap::new();

    // Process subnodes deepest-first: reversed pre-order ensures a
    // nested subnode is always processed before its parent subnode.
    for subnode in document.select("wb-subnode").iter().rev() {
        let Some(identifier) = subnode.attr("identifier") else {
            errors.push(SourceDiagnostic::error(
                Span::detached(),
                "wb-subnode is missing an identifier",
            ));
            continue;
        };
        let identifier = identifier.to_string();
        let span = spans
            .get(&identifier)
            .copied()
            .expect("bug: no span found for node identifier");
        let transclude = match subnode.attr("transclude").as_deref() {
            Some("true") => true,
            Some("false") => false,
            Some(other) => {
                errors.push(SourceDiagnostic::error(
                    span,
                    eco_format!("wb-subnode has invalid transclude value: {other:?}"),
                ));
                continue;
            }
            None => {
                errors.push(SourceDiagnostic::error(
                    span,
                    "wb-subnode is missing the transclude attribute",
                ));
                continue;
            }
        };

        let title_selection = subnode.children().first();
        if !title_selection
            .nodes()
            .first()
            .is_some_and(|n| n.has_name("wb-title"))
        {
            errors.push(SourceDiagnostic::error(
                span,
                "wb-subnode's first child must be a wb-title element",
            ));
            continue;
        }
        let title = title_selection.inner_html().to_string();
        let title_text = title_selection.text().to_string();
        title_selection.remove();

        let raw_html = subnode.inner_html().to_string();
        let mut transclusions: Vec<NodeId> = Vec::new();
        for element in subnode.select("wb-transclude").iter() {
            match element.attr("identifier").as_deref() {
                Some(id) => transclusions.push(interner.intern(id)),
                None => errors.push(SourceDiagnostic::error(
                    Span::detached(),
                    "wb-transclude is missing an identifier",
                )),
            }
        }
        let links: Vec<NodeId> = subnode
            .select("a")
            .iter()
            .filter_map(|element| {
                element
                    .attr("href")
                    .as_deref()
                    .and_then(|href| href.strip_prefix("wb:"))
                    .map(|id| interner.intern(id))
            })
            .collect();

        if transclude {
            subnode.replace_with_html(format!(
                r#"<wb-transclude identifier="{identifier}"></wb-transclude>"#
            ));
        } else {
            subnode.remove();
        }

        nodes.insert(
            interner.intern(identifier),
            (
                NodeEntry {
                    raw_html,
                    title,
                    title_text,
                    ..Default::default()
                },
                transclusions,
                links,
            ),
        );
    }

    // Extract the wb-node after subnodes have been replaced/removed.
    let mut node_iter = document.select("wb-node").iter();

    match node_iter.next() {
        None => {
            errors.push(SourceDiagnostic::error(
                Span::detached(),
                "source file produced no wb-node",
            ));
        }
        Some(wb_node) => {
            if let Some(identifier) = wb_node.attr("identifier") {
                let identifier = identifier.to_string();
                let span = spans
                    .get(&identifier)
                    .copied()
                    .expect("bug: no span found for node identifier");

                let title_selection = wb_node.children().first();
                if !title_selection
                    .nodes()
                    .first()
                    .is_some_and(|n| n.has_name("wb-title"))
                {
                    errors.push(SourceDiagnostic::error(
                        span,
                        "wb-node's first child must be a wb-title element",
                    ));
                } else {
                    let title = title_selection.inner_html().to_string();
                    let title_text = title_selection.text().to_string();
                    title_selection.remove();

                    let raw_html = wb_node.inner_html().to_string();
                    let mut transclusions: Vec<NodeId> = Vec::new();
                    for element in wb_node.select("wb-transclude").iter() {
                        match element.attr("identifier").as_deref() {
                            Some(id) => transclusions.push(interner.intern(id)),
                            None => errors.push(SourceDiagnostic::error(
                                Span::detached(),
                                "wb-transclude is missing an identifier",
                            )),
                        }
                    }
                    let links: Vec<NodeId> = wb_node
                        .select("a")
                        .iter()
                        .filter_map(|el| {
                            el.attr("href")
                                .as_deref()
                                .and_then(|href| href.strip_prefix("wb:"))
                                .map(|id| interner.intern(id))
                        })
                        .collect();

                    nodes.insert(
                        interner.intern(identifier),
                        (
                            NodeEntry {
                                raw_html,
                                title,
                                title_text,
                                ..Default::default()
                            },
                            transclusions,
                            links,
                        ),
                    );
                }
            } else {
                errors.push(SourceDiagnostic::error(
                    Span::detached(),
                    "wb-node is missing an identifier",
                ));
            }

            errors.extend(node_iter.map(|extra| {
                let span = extra
                    .attr("identifier")
                    .map(|id| {
                        spans
                            .get(id.as_ref())
                            .copied()
                            .expect("bug: no span found for wb-node identifier")
                    })
                    .unwrap_or(Span::detached());

                SourceDiagnostic::error(span, "source file produced multiple wb-node elements")
            }));
        }
    }

    if errors.is_empty() {
        Ok(nodes)
    } else {
        Err(errors)
    }
}

/// Walks `document`'s element tree once (iterative DFS), returning a map from
/// each node identifier to the span of its `wb-node` or `wb-subnode` element,
/// plus errors for any duplicate identifiers found within the document.
fn collect_node_spans(
    document: &HtmlDocument,
) -> (HashMap<String, Span>, EcoVec<SourceDiagnostic>) {
    let wb_node = HtmlTag::intern("wb-node").expect("wb-node is a valid tag");
    let wb_subnode = HtmlTag::intern("wb-subnode").expect("wb-subnode is a valid tag");
    let identifier = HtmlAttr::intern("identifier").expect("identifier is a valid attr");

    let mut spans = HashMap::new();
    let mut errors = EcoVec::new();
    let mut stack = vec![document.root()];

    while let Some(element) = stack.pop() {
        if (element.tag == wb_node || element.tag == wb_subnode)
            && let Some(id) = element.attrs.get(identifier)
        {
            match spans.entry(id.to_string()) {
                Entry::Occupied(_) => {
                    errors.push(SourceDiagnostic::error(
                        element.span,
                        eco_format!("duplicate node identifier: {id:?}"),
                    ));
                }
                Entry::Vacant(entry) => {
                    entry.insert(element.span);
                }
            }
        }
        for child in element.children.iter().rev() {
            if let HtmlNode::Element(child_elem) = child {
                stack.push(child_elem);
            }
        }
    }

    (spans, errors)
}
