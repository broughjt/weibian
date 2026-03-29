#[cfg(test)]
mod tests;

use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io;

use crate::config::{
    BACKMATTER_TEMPLATE, BuildConfig, LINK_TEMPLATE, NODE_TEMPLATE, RenderConfig,
    TRANSCLUSION_TEMPLATE,
};
use dom_query::{Document, Selection};
use ecow::{EcoVec, eco_format};
use petgraph::Direction;
use petgraph::algo::tarjan_scc;
use petgraph::graphmap::DiGraphMap;
use petgraph::visit::{Bfs, Reversed};
use typst::World;
use typst::diag::{Severity, SourceDiagnostic, Warned};
use typst::foundations::{Dict, NativeElement, Packed, Repr, Value};
use typst::introspection::{Introspector, MetadataElem};
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
        // yet.
        let nodes_result = result.and_then(|output| {
            extract(output, &mut self.interner, |node_id| {
                self.nodes.contains_key(&node_id) && self.node_to_file.get(&node_id) != Some(&id)
            })
        });

        match nodes_result {
            Ok(nodes) => {
                // Compare new title/metadata against old entries before
                // remove() clears them, so we can track which nodes had their
                // displayable backmatter information change.
                for (&node_id, (entry, _, _)) in &nodes {
                    if self.nodes.get(&node_id).is_some_and(|old| {
                        old.title != entry.title || old.metadata != entry.metadata
                    }) {
                        self.metadata_dirty.insert(node_id);
                    }
                    // New nodes are not added to metadata_dirty: no existing
                    // BackmatterCache can reference a node that didn't exist.
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
            let message = if self.removed.contains(&destination) {
                eco_format!("dangling transclusion: {name} was deleted")
            } else {
                eco_format!("dangling transclusion: {name} is not defined")
            };

            self.process_diagnostics
                .entry(file_id)
                .or_default()
                .push(SourceDiagnostic::warning(Span::detached(), message));
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
            let message = if self.removed.contains(&destination) {
                eco_format!("dangling link: {name} was deleted")
            } else {
                eco_format!("dangling link: {name} is not defined")
            };

            self.process_diagnostics
                .entry(file_id)
                .or_default()
                .push(SourceDiagnostic::warning(Span::detached(), message));
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
        let metadata_dirty = std::mem::take(&mut self.metadata_dirty);
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

        // Backmatter computation (part of Pass 1).
        //
        // Compute backmatter sets for every node in the transclusion graph,
        // then diff against the cached sets to determine which nodes need their
        // backmatter re-rendered.
        //
        // `outlinks_accum[a]` = direct links from `a` ∪ outlinks of every
        // node that `a` transitively transcludes. Because `sccs` is in reverse
        // topological order (leaves first), each transclusion target's entry is
        // already present when we visit the parent.
        let mut outlinks_accum: HashMap<NodeId, BTreeSet<NodeId>> = HashMap::new();
        let mut backmatter_render: HashSet<NodeId> = HashSet::new();

        for scc in &sccs {
            let id = scc[0];
            // Removed nodes remain as targets in the transclusion graph until
            // the edge from their parent is cleared; skip them here.
            if !self.nodes.contains_key(&id) || unrenderable.contains(&id) {
                continue;
            }

            let mut outlinks: BTreeSet<NodeId> = self.links.neighbors(id).collect();
            for target in self.transclusions.neighbors(id) {
                if let Some(acc) = outlinks_accum.get(&target) {
                    outlinks.extend(acc.iter().copied());
                }
            }
            outlinks_accum.insert(id, outlinks.clone());

            let contexts: BTreeSet<NodeId> = self
                .transclusions
                .neighbors_directed(id, Direction::Incoming)
                .collect();
            let backlinks: BTreeSet<NodeId> = self
                .links
                .neighbors_directed(id, Direction::Incoming)
                .collect();

            let new_cache = BackmatterCache {
                contexts,
                backlinks,
                outlinks,
            };

            let entry = self
                .nodes
                .get_mut(&id)
                .expect("bug: node in scc has no entry");
            let needs_rerender = match &entry.backmatter_cache {
                None => true,
                Some(old) => {
                    old.contexts != new_cache.contexts
                        || old.backlinks != new_cache.backlinks
                        || old.outlinks != new_cache.outlinks
                        || new_cache
                            .contexts
                            .iter()
                            .chain(new_cache.backlinks.iter())
                            .chain(new_cache.outlinks.iter())
                            .any(|member| {
                                metadata_dirty.contains(member) || removed.contains(member)
                            })
                }
            };

            entry.backmatter_cache = Some(new_cache);

            if needs_rerender {
                backmatter_render.insert(id);
            }
        }

        // Also check isolated nodes (not in the transclusion graph) that are
        // in the render set. Their contexts and backlinks may have changed even
        // though they have no transclusion edges.
        for &id in &dirty {
            if self.transclusions.contains_node(id) || unrenderable.contains(&id) {
                continue;
            }
            let outlinks: BTreeSet<NodeId> = self.links.neighbors(id).collect();
            let contexts: BTreeSet<NodeId> = self
                .transclusions
                .neighbors_directed(id, Direction::Incoming)
                .collect();
            let backlinks: BTreeSet<NodeId> = self
                .links
                .neighbors_directed(id, Direction::Incoming)
                .collect();
            let new_cache = BackmatterCache {
                contexts,
                backlinks,
                outlinks,
            };
            let entry = self
                .nodes
                .get_mut(&id)
                .expect("bug: dirty node has no entry");
            let needs_rerender = match &entry.backmatter_cache {
                None => true,
                Some(old) => {
                    old.contexts != new_cache.contexts
                        || old.backlinks != new_cache.backlinks
                        || old.outlinks != new_cache.outlinks
                        || new_cache
                            .contexts
                            .iter()
                            .chain(new_cache.backlinks.iter())
                            .chain(new_cache.outlinks.iter())
                            .any(|member| {
                                metadata_dirty.contains(member) || removed.contains(member)
                            })
                }
            };
            entry.backmatter_cache = Some(new_cache);
            if needs_rerender {
                backmatter_render.insert(id);
            }
        }

        // Nodes that need only backmatter re-rendered (body unchanged) are
        // appended to render_order after the topo-ordered body nodes. Their
        // relative order doesn't matter since backmatter rendering has no
        // inter-node dependencies.
        render_order.extend(
            backmatter_render
                .iter()
                .copied()
                .filter(|id| !render.contains(id) && !unrenderable.contains(id)),
        );

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
            if render.contains(&id) {
                let raw_html = self.nodes[&id].raw_html.as_str();
                let document = Document::from(raw_html);

                // Render internal links. Done before transclusion substitution so
                // that links inside already-rendered transclusion bodies are not
                // double-processed.
                if self.links.neighbors(id).next().is_some() {
                    let links = document.select("a").iter().filter_map(|element| {
                        element
                            .attr("href")
                            .and_then(|href| href.strip_prefix("wb:").map(ToOwned::to_owned))
                            .map(|identifier| (element, identifier))
                    });
                    for (element, identifier) in links {
                        let counter: u32 = element
                            .attr("data-counter")
                            .expect("bug: link is missing a data-counter")
                            .parse()
                            .expect("bug: link has invalid data-counter");
                        let href = minijinja::Value::from_safe_string(config.href(&identifier));
                        let content = element.inner_html().to_string();
                        let link_metadata = self.nodes[&id]
                            .link_metadata
                            .get(&counter)
                            .cloned()
                            .unwrap_or_default();
                        let context = if let Some(target_id) = self.interner.get(&identifier)
                            && let Some(entry) = self.nodes.get(&target_id)
                        {
                            minijinja::context! {
                                link => minijinja::context! {
                                    identifier => identifier,
                                    href => &href,
                                    content => content,
                                    resolved => true,
                                    title => entry.title.as_str(),
                                    title_text => entry.title_text.as_str(),
                                    metadata => entry.metadata,
                                    link_metadata => link_metadata,
                                },
                                site => site_context,
                            }
                        } else {
                            minijinja::context! {
                                link => minijinja::context! {
                                    identifier => identifier,
                                    href => href,
                                    content => content,
                                    resolved => false,
                                    link_metadata => link_metadata,
                                },
                                site => site_context,
                            }
                        };
                        let replacement = link_template.render(context).map_err(|e| {
                            anyhow::anyhow!("failed to render link template for {identifier}: {e}")
                        })?;

                        element.replace_with_html(replacement);
                    }
                }

                if self.transclusions.neighbors(id).next().is_some() {
                    for element in document.select("wb-transclude").iter() {
                        let identifier = element
                            .attr("identifier")
                            .expect("bug: wb-transclude is missing an identifier");
                        let counter: u32 = element
                            .attr("counter")
                            .expect("bug: wb-transclude is missing a counter")
                            .parse()
                            .expect("bug: wb-transclude has invalid counter");
                        let transclusion_metadata = self.nodes[&id]
                            .transclusion_metadata
                            .get(&counter)
                            .cloned()
                            .unwrap_or_default();
                        let transclude_id = self
                            .interner
                            .get(identifier.as_ref())
                            .expect("bug: wb-transclude identifier was not interned");
                        let context = if let Some(entry) = self.nodes.get(&transclude_id) {
                            let body = entry
                                .rendered_body
                                .as_deref()
                                .expect("bug: wb-transclude target has no rendered_body");

                            minijinja::context! {
                                transclusion => minijinja::context! {
                                    identifier => identifier.as_ref(),
                                    href => minijinja::Value::from_safe_string(config.href(identifier.as_ref())),
                                    resolved => true,
                                    title => entry.title.as_str(),
                                    title_text => entry.title_text.as_str(),
                                    body => body,
                                    metadata => entry.metadata,
                                    transclusion_metadata => transclusion_metadata,
                                },
                                site => site_context,
                            }
                        } else {
                            minijinja::context! {
                                transclusion => minijinja::context! {
                                    identifier => identifier.as_ref(),
                                    resolved => false,
                                    transclusion_metadata => transclusion_metadata,
                                },
                                site => site_context,
                            }
                        };
                        let replacement = transclusion_template.render(context).map_err(|e| {
                            anyhow::anyhow!(
                                "failed to render transclusion template for {identifier}: {e}"
                            )
                        })?;

                        element.replace_with_html(replacement);
                    }
                }

                let rendered_body = document.select("body").first().inner_html().to_string();
                self.nodes.get_mut(&id).unwrap().rendered_body = Some(rendered_body);
            }

            if backmatter_render.contains(&id) {
                let cache = self.nodes[&id]
                    .backmatter_cache
                    .as_ref()
                    .expect("bug: backmatter_render node has no backmatter_cache");

                let node_info = |node_id: NodeId| {
                    let name = self.interner.name(node_id);
                    let entry = self.nodes.get(&node_id);
                    minijinja::context! {
                        id => name,
                        href => minijinja::Value::from_safe_string(config.href(name)),
                        title => entry.map(|e| e.title.as_str()).unwrap_or(""),
                        title_text => entry.map(|e| e.title_text.as_str()).unwrap_or(""),
                        metadata => entry.map(|e| &e.metadata),
                    }
                };

                let contexts: Vec<_> = cache.contexts.iter().copied().map(&node_info).collect();
                let backlinks: Vec<_> = cache.backlinks.iter().copied().map(&node_info).collect();
                let outlinks: Vec<_> = cache.outlinks.iter().copied().map(&node_info).collect();

                let name = self.interner.name(id);
                let rendered_backmatter = backmatter_template
                    .render(minijinja::context! {
                        backmatter => minijinja::context! {
                            contexts => contexts,
                            backlinks => backlinks,
                            outlinks => outlinks,
                        },
                        node => minijinja::context! {
                            id => name,
                            href => minijinja::Value::from_safe_string(config.href(name)),
                        },
                        site => site_context,
                    })
                    .map_err(|e| {
                        anyhow::anyhow!("failed to render backmatter template for {name}: {e}")
                    })?;

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
                // TODO: We should panic if rendered backmatter is missing at this point, right?
                let backmatter = entry.rendered_backmatter.as_deref().unwrap_or_default();
                let html = node_template
                    .render(minijinja::context! {
                        node => minijinja::context! {
                            id => name,
                            href => minijinja::Value::from_safe_string(config.href(name)),
                            title => entry.title.as_str(),
                            title_text => entry.title_text.as_str(),
                            body => body,
                            backmatter => backmatter,
                            metadata => entry.metadata,
                        },
                        site => site_context,
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
        let Warned {
            output: result,
            mut warnings,
        } = typst::compile::<HtmlDocument>(&self.0);

        // Discard warnings about html being an unstable feature, html is kind
        // of the whole game here
        warnings.retain(|diagnostic: &mut SourceDiagnostic| {
            !(diagnostic.severity == Severity::Warning && diagnostic.message == HTML_MESSAGE)
        });

        let output = result.and_then(|html_document| {
            typst_html::html(&html_document).map(|html| {
                let (spans, span_errors) = collect_node_spans(&html_document);
                let (metadata, transclusion_metadata, link_metadata, meta_errors) =
                    collect_metadata(html_document.introspector().as_ref(), &spans);
                let mut errors = span_errors;
                errors.extend(meta_errors);

                CompileOutput {
                    html,
                    spans,
                    metadata,
                    transclusion_metadata,
                    link_metadata,
                    errors,
                }
            })
        });

        Warned { output, warnings }
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

/// Parses the HTML in `output` into a map of node IDs to node entries.
///
/// Subnodes are replaced with `<wb-transclude>` references (or removed) as a
/// side effect of extraction. `node_exists` is called to detect cross-file
/// duplicate identifiers.
///
/// Returns `Err` with all collected diagnostics if any validation errors occur,
/// or `Ok` with the node map on success.
fn extract(
    output: CompileOutput,
    interner: &mut NodeInterner,
    node_exists: impl Fn(NodeId) -> bool,
) -> Result<HashMap<NodeId, ExtractOutput>, EcoVec<SourceDiagnostic>> {
    let CompileOutput {
        html,
        spans,
        mut metadata,
        mut transclusion_metadata,
        mut link_metadata,
        mut errors,
    } = output;
    let document = Document::from(html);
    let mut nodes = HashMap::with_capacity(spans.len());
    let mut synthetic_counter: u32 = transclusion_metadata.keys().copied().max().map_or(0, |m| {
        m.checked_add(1).expect("transclusion counter overflow")
    });

    // Check for global duplicate identifiers
    errors.extend(
        spans
            .iter()
            .filter(|(id, _)| node_exists(interner.intern(*id)))
            .map(|(id, &span)| {
                SourceDiagnostic::error(span, eco_format!("duplicate node identifier: {id:?}"))
            }),
    );

    // Process subnodes deepest-first: reversed pre-order ensures a
    // nested subnode is always processed before its parent subnode.
    for subnode in document.select("wb-subnode").iter().rev() {
        let Some((identifier, entry)) = extract_node_content(
            &subnode,
            true,
            &spans,
            interner,
            &mut metadata,
            &mut transclusion_metadata,
            &mut link_metadata,
            &mut errors,
        ) else {
            continue;
        };
        let transclude = match subnode.attr("transclude").as_deref() {
            Some("true") => true,
            Some("false") => false,
            Some(other) => {
                errors.push(SourceDiagnostic::error(
                    entry.0.span,
                    eco_format!("wb-subnode has invalid transclude value: {other:?}"),
                ));
                continue;
            }
            None => {
                errors.push(SourceDiagnostic::error(
                    entry.0.span,
                    "wb-subnode is missing the transclude attribute",
                ));
                continue;
            }
        };

        if transclude {
            let counter = synthetic_counter;
            synthetic_counter = synthetic_counter
                .checked_add(1)
                .expect("transclusion counter overflow");

            transclusion_metadata.insert(counter, entry.0.metadata.clone());
            subnode.replace_with_html(format!(
                r#"<wb-transclude identifier="{identifier}" counter="{counter}"></wb-transclude>"#
            ));
        } else {
            subnode.remove();
        }

        nodes.insert(interner.intern(identifier), entry);
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
            if let Some((identifier, entry)) = extract_node_content(
                &wb_node,
                false,
                &spans,
                interner,
                &mut metadata,
                &mut transclusion_metadata,
                &mut link_metadata,
                &mut errors,
            ) {
                nodes.insert(interner.intern(identifier), entry);
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

    assert!(
        metadata.is_empty(),
        "bug: unconsumed node metadata: {:?}",
        metadata.keys().collect::<Vec<_>>()
    );
    assert!(
        transclusion_metadata.is_empty(),
        "bug: unconsumed transclusion metadata: {:?}",
        transclusion_metadata.keys().collect::<Vec<_>>()
    );
    assert!(
        link_metadata.is_empty(),
        "bug: unconsumed link metadata: {:?}",
        link_metadata.keys().collect::<Vec<_>>()
    );

    if errors.is_empty() {
        Ok(nodes)
    } else {
        Err(errors)
    }
}

/// Extracts the content of a `wb-node` or `wb-subnode` element into a
/// [`NodeEntry`], collecting its transclusions and links and consuming its
/// metadata from the provided map.
///
/// Returns `None` (pushing an error) if the identifier attribute is missing or
/// if the element's first child is not a `wb-title` element.
#[allow(clippy::too_many_arguments)]
fn extract_node_content(
    element: &Selection,
    is_subnode: bool,
    spans: &HashMap<String, Span>,
    interner: &mut NodeInterner,
    metadata: &mut HashMap<String, HashMap<String, Vec<String>>>,
    transclusion_metadata: &mut HashMap<u32, HashMap<String, Vec<String>>>,
    link_metadata: &mut HashMap<u32, HashMap<String, Vec<String>>>,
    errors: &mut EcoVec<SourceDiagnostic>,
) -> Option<(String, ExtractOutput)> {
    let Some(identifier) = element.attr("identifier") else {
        errors.push(SourceDiagnostic::error(
            Span::detached(),
            if is_subnode {
                "wb-subnode is missing an identifier"
            } else {
                "wb-node is missing an identifier"
            },
        ));

        return None;
    };
    let identifier = identifier.to_string();
    let span = spans
        .get(&identifier)
        .copied()
        .expect("bug: no span found for node identifier");

    let title_selection = element.children().first();
    if !title_selection
        .nodes()
        .first()
        .is_some_and(|n| n.has_name("wb-title"))
    {
        errors.push(SourceDiagnostic::error(
            span,
            if is_subnode {
                "wb-subnode's first child must be a wb-title element"
            } else {
                "wb-node's first child must be a wb-title element"
            },
        ));
        return None;
    }
    let title = title_selection.inner_html().to_string();
    let title_text = title_selection.text().to_string();
    title_selection.remove();

    let raw_html = element.inner_html().to_string();

    let mut transclusions: Vec<NodeId> = Vec::new();
    let mut node_transclusion_metadata: HashMap<u32, HashMap<String, Vec<String>>> = HashMap::new();
    for wb_transclude in element.select("wb-transclude").iter() {
        let id = match wb_transclude.attr("identifier").as_deref() {
            Some(id) => id.to_owned(),
            None => {
                errors.push(SourceDiagnostic::error(
                    Span::detached(),
                    "wb-transclude is missing an identifier",
                ));
                continue;
            }
        };
        let counter = match wb_transclude.attr("counter").as_deref() {
            Some(n) => match n.parse::<u32>() {
                Ok(n) => n,
                Err(_) => {
                    errors.push(SourceDiagnostic::error(
                        Span::detached(),
                        eco_format!("wb-transclude has invalid counter: {n:?}"),
                    ));
                    continue;
                }
            },
            None => {
                errors.push(SourceDiagnostic::error(
                    Span::detached(),
                    "wb-transclude is missing a counter attribute",
                ));
                continue;
            }
        };

        if let Some(metadata) = transclusion_metadata.remove(&counter) {
            node_transclusion_metadata.insert(counter, metadata);
        }
        transclusions.push(interner.intern(&id));
    }

    let mut links: Vec<NodeId> = Vec::new();
    let mut node_link_metadata: HashMap<u32, HashMap<String, Vec<String>>> = HashMap::new();
    let links_iter = element.select("a").iter().filter_map(|element| {
        element
            .attr("href")
            .and_then(|href| href.strip_prefix("wb:").map(ToOwned::to_owned))
            .map(|identifier| (element, identifier))
    });
    for (anchor, id) in links_iter {
        let counter = match anchor.attr("data-counter").as_deref() {
            Some(n) => match n.parse::<u32>() {
                Ok(n) => n,
                Err(_) => {
                    errors.push(SourceDiagnostic::error(
                        Span::detached(),
                        eco_format!("link has invalid data-counter: {n:?}"),
                    ));
                    continue;
                }
            },
            None => {
                errors.push(SourceDiagnostic::error(
                    Span::detached(),
                    "link is missing a data-counter attribute",
                ));
                continue;
            }
        };

        if let Some(metadata) = link_metadata.remove(&counter) {
            node_link_metadata.insert(counter, metadata);
        }
        links.push(interner.intern(id));
    }

    let node_metadata = metadata.remove(&identifier).unwrap_or_default();

    Some((
        identifier,
        (
            NodeEntry {
                raw_html,
                title,
                title_text,
                span,
                metadata: node_metadata,
                transclusion_metadata: node_transclusion_metadata,
                link_metadata: node_link_metadata,
                ..Default::default()
            },
            transclusions,
            links,
        ),
    ))
}

/// Queries the introspector for `#metadata(...)` elements that carry node or
/// transclusion call-site metadata, and returns them as two separate maps.
///
/// Metadata elements are identified by a `wb-metadata` key whose value is a
/// two-element array `[kind, discriminant]`:
/// - `["node", identifier]`      — node/subnode metadata, keyed by identifier string
/// - `["transclude", counter]`   — transclusion call-site metadata, keyed by counter integer
/// - `["link", counter]`         — link call-site metadata, keyed by counter integer
///
/// Errors are pushed for:
/// - `wb-metadata` present but not a two-element array of the expected shape
/// - node identifier not present in `spans` (unknown node)
/// - duplicate entries for the same node or counter
#[allow(clippy::type_complexity)]
fn collect_metadata<I: Introspector>(
    introspector: &I,
    spans: &HashMap<String, Span>,
) -> (
    HashMap<String, HashMap<String, Vec<String>>>,
    HashMap<u32, HashMap<String, Vec<String>>>,
    HashMap<u32, HashMap<String, Vec<String>>>,
    EcoVec<SourceDiagnostic>,
) {
    let selector = MetadataElem::ELEM.select();
    let items = introspector.query(&selector);
    let mut errors = EcoVec::new();
    let mut node_result: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
    let mut transclusion_result: HashMap<u32, HashMap<String, Vec<String>>> = HashMap::new();
    let mut link_result: HashMap<u32, HashMap<String, Vec<String>>> = HashMap::new();

    for item in &items {
        let Some((dictionary, wb_metadata)) =
            Packed::<MetadataElem>::from_ref(item).and_then(|meta| match &meta.value {
                Value::Dict(dictionary) => dictionary
                    .get("wb-metadata")
                    .ok()
                    .map(|wb_metadata| (dictionary, wb_metadata)),
                _ => None,
            })
        else {
            continue;
        };
        let Value::Array(array) = wb_metadata else {
            errors.push(SourceDiagnostic::error(
                item.span(),
                "\"wb-metadata\" must be a [kind, discriminant] array",
            ));
            continue;
        };

        let mut iter = array.iter();
        match (iter.next(), iter.next()) {
            (Some(Value::Str(kind)), Some(discriminant)) => match kind.as_str() {
                "node" => {
                    let Value::Str(identifier) = discriminant else {
                        errors.push(SourceDiagnostic::error(
                            item.span(),
                            "\"wb-metadata\" node identifier must be a string",
                        ));
                        continue;
                    };
                    let identifier = identifier.to_string();

                    if !spans.contains_key(&identifier) {
                        errors.push(SourceDiagnostic::error(
                            item.span(),
                            eco_format!("metadata for unknown node: {identifier:?}"),
                        ));
                        continue;
                    }

                    match node_result.entry(identifier) {
                        Entry::Vacant(entry) => {
                            entry.insert(normalize_metadata(dictionary));
                        }
                        Entry::Occupied(e) => {
                            errors.push(SourceDiagnostic::error(
                                item.span(),
                                eco_format!("duplicate metadata for node: {:?}", e.key()),
                            ));
                        }
                    }
                }
                "transclude" => {
                    let Value::Int(counter_i64) = discriminant else {
                        errors.push(SourceDiagnostic::error(
                            item.span(),
                            "\"wb-metadata\" transclude counter must be an integer",
                        ));
                        continue;
                    };
                    let counter = match u32::try_from(*counter_i64) {
                        Ok(n) => n,
                        Err(_) => {
                            errors.push(SourceDiagnostic::error(
                                item.span(),
                                eco_format!(
                                    "\"wb-metadata\" transclude counter out of range: {counter_i64}"
                                ),
                            ));
                            continue;
                        }
                    };

                    match transclusion_result.entry(counter) {
                        Entry::Vacant(entry) => {
                            entry.insert(normalize_metadata(dictionary));
                        }
                        Entry::Occupied(entry) => {
                            errors.push(SourceDiagnostic::error(
                                item.span(),
                                eco_format!(
                                    "duplicate metadata for transclusion counter: {}",
                                    entry.key()
                                ),
                            ));
                        }
                    }
                }
                "link" => {
                    let Value::Int(counter_i64) = discriminant else {
                        errors.push(SourceDiagnostic::error(
                            item.span(),
                            "\"wb-metadata\" link counter must be an integer",
                        ));
                        continue;
                    };
                    let counter = match u32::try_from(*counter_i64) {
                        Ok(n) => n,
                        Err(_) => {
                            errors.push(SourceDiagnostic::error(
                                item.span(),
                                eco_format!(
                                    "\"wb-metadata\" link counter out of range: {counter_i64}"
                                ),
                            ));
                            continue;
                        }
                    };

                    match link_result.entry(counter) {
                        Entry::Vacant(entry) => {
                            entry.insert(normalize_metadata(dictionary));
                        }
                        Entry::Occupied(entry) => {
                            errors.push(SourceDiagnostic::error(
                                item.span(),
                                eco_format!("duplicate metadata for link counter: {}", entry.key()),
                            ));
                        }
                    }
                }
                _ => {
                    errors.push(SourceDiagnostic::error(
                        item.span(),
                        eco_format!("unknown \"wb-metadata\" kind: {:?}", kind.as_str()),
                    ));
                }
            },
            _ => {
                errors.push(SourceDiagnostic::error(
                    item.span(),
                    "\"wb-metadata\" must be a two-element [kind, discriminant] array",
                ));
            }
        }
    }

    (node_result, transclusion_result, link_result, errors)
}

/// Converts a Typst metadata dict into a `HashMap<String, Vec<String>>`,
/// skipping the structural `"wb-metadata"` key.
///
/// Values are normalised as follows:
/// - `none`  → key omitted entirely
/// - string  → single-element vec
/// - array   → vec of elements, with `none` items dropped and non-strings converted via `repr()`
/// - anything else → single-element vec containing the `repr()` string
// TODO: Maybe use EcoVec here
fn normalize_metadata(dictionary: &Dict) -> HashMap<String, Vec<String>> {
    let mut result = HashMap::with_capacity(dictionary.len().saturating_sub(1));

    for (key, value) in dictionary.iter() {
        if key.as_str() == "wb-metadata" {
            continue;
        }
        let values: Vec<String> = match value {
            Value::None => continue,
            Value::Str(s) => vec![s.to_string()],
            Value::Array(a) => a
                .iter()
                .filter_map(|v| match v {
                    Value::None => None,
                    Value::Str(s) => Some(s.to_string()),
                    other => Some(other.repr().to_string()),
                })
                .collect(),
            other => vec![other.repr().to_string()],
        };
        if !values.is_empty() {
            result.insert(key.to_string(), values);
        }
    }

    result
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
