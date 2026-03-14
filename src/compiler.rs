use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;

use dom_query::Document;
use ecow::{EcoVec, eco_format};
use typst::World;
use typst::diag::{Severity, SourceDiagnostic, Warned};
use typst::syntax::{FileId, Span};
use typst_html::{HtmlAttr, HtmlDocument, HtmlNode, HtmlTag};

const HTML_MESSAGE: &str = "html export is under active development and incomplete";

/// Compiles Typst source files into nodes and maintains the in-memory node
/// store and per-file diagnostics across incremental rebuilds.
#[derive(Default)]
pub struct Compiler {
    interner: NodeInterner,
    files: HashMap<FileId, Vec<NodeId>>,
    nodes: HashMap<NodeId, NodeEntry>,
    file_diagnostics: HashMap<FileId, (EcoVec<SourceDiagnostic>, EcoVec<SourceDiagnostic>)>,
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
        // Clean up any nodes from a previous compilation of this file.
        let old_ids: Vec<NodeId> = self.files.remove(&id).unwrap_or_default();

        for old_id in &old_ids {
            self.nodes.remove(old_id);
        }

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
                let interner = &mut self.interner;
                let nodes = &self.nodes;
                extract(&html_document, document, interner, |id| {
                    nodes.contains_key(&id)
                })
            })
        });

        match nodes_result {
            Ok(nodes) => {
                // Old IDs absent from the new compilation are orphaned.
                for old_id in old_ids {
                    if !nodes.contains_key(&old_id) {
                        self.dirty.remove(&old_id);
                        self.removed.insert(old_id);
                    }
                }
                // Newly compiled IDs are dirty.
                for &new_id in nodes.keys() {
                    self.removed.remove(&new_id);
                    self.dirty.insert(new_id);
                }

                self.files.insert(id, nodes.keys().copied().collect());
                self.nodes.extend(nodes);

                if warnings.is_empty() {
                    self.file_diagnostics.remove(&id);
                } else {
                    self.file_diagnostics.insert(id, (warnings, EcoVec::new()));
                }
            }
            Err(errors) => {
                // When compilation fails, all old IDs are orphaned
                for old_id in old_ids {
                    self.dirty.remove(&old_id);
                    self.removed.insert(old_id);
                }
                self.file_diagnostics.insert(id, (warnings, errors));
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
            for old_id in &old_ids {
                self.nodes.remove(old_id);
                self.dirty.remove(old_id);
            }

            self.removed.extend(old_ids);
        }

        self.file_diagnostics.remove(&id);
    }

    /// Returns all collected file diagnostics, keyed by source [`FileId`].
    ///
    /// Each entry is a `(warnings, errors)` pair of [`SourceDiagnostic`] vecs.
    pub fn file_diagnostics(
        &self,
    ) -> &HashMap<FileId, (EcoVec<SourceDiagnostic>, EcoVec<SourceDiagnostic>)> {
        &self.file_diagnostics
    }

    /// Returns an [`OutputPlan`] describing the writes and deletes to apply to
    /// the output directory, and clears the dirty and removed sets.
    pub fn process(&mut self) -> OutputPlan {
        let dirty = std::mem::take(&mut self.dirty);
        let writes = dirty
            .into_iter()
            .filter_map(|id| {
                let name = self.interner.name(id).to_string();
                self.nodes
                    .get(&id)
                    .map(|entry| (name, entry.raw_html.clone()))
            })
            .collect();
        let deletes = std::mem::take(&mut self.removed)
            .into_iter()
            .map(|id| self.interner.name(id).to_string())
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
    pub transclusions: Vec<NodeId>,
    pub links: Vec<NodeId>,
    pub rendered_body: Option<String>,
    pub rendered_backmatter: Option<String>,
}

/// The set of writes and deletes to apply to the output directory after a
/// process call.
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
) -> Result<HashMap<NodeId, NodeEntry>, EcoVec<SourceDiagnostic>> {
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

    let mut nodes: HashMap<NodeId, NodeEntry> = HashMap::new();

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

        let html = subnode.html().to_string();
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
            NodeEntry {
                raw_html: html,
                transclusions,
                links,
                ..Default::default()
            },
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
                    NodeEntry {
                        raw_html: wb_node.html().to_string(),
                        transclusions,
                        links,
                        ..Default::default()
                    },
                );
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
