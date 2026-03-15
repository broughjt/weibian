use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;

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
    files: HashMap<FileId, Vec<NodeId>>,
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
                    self.removed.remove(&node_id);
                    self.dirty.insert(node_id);
                    for &t in transclusions {
                        self.transclusions.add_edge(node_id, t, ());
                    }
                    for &l in links {
                        self.links.add_edge(node_id, l, ());
                    }
                }

                self.files.insert(id, nodes.keys().copied().collect());
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
        if let Some(old_ids) = self.files.remove(&id) {
            for old_id in old_ids {
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
    pub fn process(&mut self) -> OutputPlan {
        if self.dirty.is_empty() && self.removed.is_empty() {
            return OutputPlan {
                writes: HashMap::new(),
                deletes: HashSet::new(),
            };
        }

        self.process_diagnostics.clear();

        let dirty = std::mem::take(&mut self.dirty);
        let removed = std::mem::take(&mut self.removed);

        let reversed = Reversed(&self.transclusions);

        let mut dirty_rerender: HashSet<NodeId> = HashSet::new();
        for &start in &dirty {
            let mut bfs = Bfs::new(reversed, start);
            while let Some(id) = bfs.next(reversed) {
                dirty_rerender.insert(id);
            }
        }

        let mut removed_reachable: HashSet<NodeId> = HashSet::new();
        for &start in &removed {
            let mut bfs = Bfs::new(reversed, start);
            while let Some(id) = bfs.next(reversed) {
                removed_reachable.insert(id);
            }
        }

        // Removed nodes themselves are deleted, not re-rendered; only their ancestors are.
        let rerender = &dirty_rerender | &(&removed_reachable - &removed);

        let sccs = tarjan_scc(&self.transclusions);

        // Build reverse map: node → file, for attributing cycle errors.
        // TODO: Probably we should have this in compiler state
        let node_to_file: HashMap<NodeId, FileId> = self
            .files
            .iter()
            .flat_map(|(&file_id, node_ids)| node_ids.iter().map(move |&nid| (nid, file_id)))
            .collect();

        // Pass 1: detect cycles and compute the transitively unrenderable set.
        //
        // SCCs are in reverse topological order (leaves first), so when we
        // visit SCC[i], every node it could transclude has already been
        // processed---if any target is unrenderable, we catch it here.
        let mut unrenderable: HashSet<NodeId> = HashSet::new();
        for scc in &sccs {
            let is_cyclic = scc.len() > 1 || self.transclusions.contains_edge(scc[0], scc[0]);

            if is_cyclic {
                let names: Vec<&str> = scc.iter().map(|&id| self.interner.name(id)).collect();
                let message = eco_format!("transclusion cycle: {}", names.join(", "));

                unrenderable.extend(scc.iter().copied());

                let files_in_cycle: HashSet<FileId> = scc
                    .iter()
                    .map(|&id| {
                        *node_to_file
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
            } else if scc.iter().any(|&id| {
                self.transclusions
                    .neighbors(id)
                    .any(|neighbor| unrenderable.contains(&neighbor))
            }) {
                unrenderable.extend(scc);
            }
        }

        // Warn on dangling transclusions (target node does not exist).
        for (source, destination, _) in self
            .transclusions
            .all_edges()
            .filter(|&(_, destination, _)| !self.nodes.contains_key(&destination))
        {
            let file_id = node_to_file
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
            let file_id = node_to_file
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

        // Pass 2: render wb-transclude substitutions in topological order (leaves first).
        for scc in &sccs {
            let id = scc[0];

            if unrenderable.contains(&id) || !rerender.contains(&id) {
                continue;
            }

            let raw_html = self.nodes[&id].raw_html.as_str();
            let document = Document::from(raw_html);

            for element in document.select("a").iter() {
                if let Some(href) = element.attr("href")
                    && let Some(node_id) = href.strip_prefix("wb:")
                {
                    // TODO: Support configurable index node, root
                    // directory, and trailing slash.
                    element.set_attr("href", &format!("/{node_id}.html"));
                }
            }

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
                        "<section class=\"block\"><details open><h1>{}</h1>{body}</details></section>",
                        entry.title
                    );
                    element.replace_with_html(replacement);
                } else {
                    element.remove();
                }
            }

            let rendered = document.select("body").first().inner_html().to_string();
            self.nodes.get_mut(&id).unwrap().rendered_body = Some(rendered);
        }

        // Isolated nodes (no transclusion edges) are absent from the SCC list.
        // They have no wb-transclude placeholders, so we just need to render
        // anchor tags
        for &id in &rerender {
            if !self.transclusions.contains_node(id) {
                let raw_html = self.nodes[&id].raw_html.as_str();
                let document = Document::from(raw_html);

                for element in document.select("a").iter() {
                    if let Some(href) = element.attr("href")
                        && let Some(node_id) = href.strip_prefix("wb:")
                    {
                        // TODO: Support configurable index node, root
                        // directory, and trailing slash.
                        element.set_attr("href", &format!("/{node_id}.html"));
                    }
                }

                self.nodes.get_mut(&id).unwrap().rendered_body =
                    Some(document.select("body").first().inner_html().to_string());
            }
        }

        let writes = (&rerender - &unrenderable)
            .into_iter()
            .map(|id| {
                let name = self.interner.name(id).to_string();
                let html = self.nodes[&id]
                    .rendered_body
                    .clone()
                    .expect("bug: renderable node has no rendered_body after pass 2");
                (name, html)
            })
            .collect();

        let deletes = removed
            .iter()
            .chain(unrenderable.intersection(&rerender))
            .map(|&id| self.interner.name(id).to_string())
            .collect();

        OutputPlan { writes, deletes }
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
) -> Result<HashMap<NodeId, (NodeEntry, Vec<NodeId>, Vec<NodeId>)>, EcoVec<SourceDiagnostic>> {
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
