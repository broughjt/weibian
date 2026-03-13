use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::Path;

use dom_query::Document;
use ecow::{EcoVec, eco_format};
use typst::World;
use typst::diag::{Severity, SourceDiagnostic, Warned};
use typst::syntax::{FileId, Span};
use typst_html::{HtmlAttr, HtmlDocument, HtmlNode, HtmlTag};

const HTML_MESSAGE: &str = "html export is under active development and incomplete";

pub type NodeId = String;

/// Compiles Typst source files into nodes and maintains the in-memory node
/// store and per-file diagnostics across incremental rebuilds.
pub struct Compiler {
    files: HashMap<FileId, Vec<NodeId>>,
    // TODO: Don't keep span in here if we don't end up needing it in process
    nodes: HashMap<NodeId, (String, Span)>,
    file_diagnostics: HashMap<FileId, (EcoVec<SourceDiagnostic>, EcoVec<SourceDiagnostic>)>,
    // TODO: Definitely won't be just a Vec<String>, should map NodeIds to a a list of SourceDiagnostics
    // node_diagnostics: Vec<String>,
}

impl Compiler {
    /// Creates an empty `Compiler`.
    pub fn new() -> Self {
        Self {
            files: HashMap::new(),
            nodes: HashMap::new(),
            file_diagnostics: HashMap::new(),
        }
    }

    /// Compiles a single source file and splits it into nodes, updating the
    /// node store and diagnostics.
    ///
    /// Typst compile errors and node-splitting errors (e.g. duplicate node IDs)
    /// are stored as diagnostics rather than returned as `Err`. `Err` is
    /// reserved for I/O failures and other fatal conditions.
    pub fn compile<W: World>(&mut self, world: &W, id: FileId) -> anyhow::Result<()> {
        // Clean up any nodes from a previous compilation of this file.
        if let Some(old_ids) = self.files.remove(&id) {
            for old_id in &old_ids {
                self.nodes.remove(old_id);
            }
        }

        let Warned {
            output: result,
            mut warnings,
        } = typst::compile::<HtmlDocument>(world);

        // Discard warnings about html being an unstable feature, html is kind
        // of the whole game here
        warnings.retain(|d: &mut SourceDiagnostic| {
            !(d.severity == Severity::Warning && d.message == HTML_MESSAGE)
        });

        match result {
            Ok(html_document) => match typst_html::html(&html_document) {
                Ok(content) => {
                    let document = Document::from(content);

                    match extract(&html_document, document, |id| self.nodes.contains_key(id)) {
                        Ok(nodes) => {
                            self.files.insert(id, nodes.keys().cloned().collect());
                            self.nodes.extend(nodes);

                            if warnings.is_empty() {
                                self.file_diagnostics.remove(&id);
                            } else {
                                self.file_diagnostics.insert(id, (warnings, EcoVec::new()));
                            }
                        }
                        Err(errors) => {
                            self.file_diagnostics.insert(id, (warnings, errors));
                        }
                    }
                }
                Err(errors) => {
                    self.file_diagnostics.insert(id, (warnings, errors));
                }
            },
            Err(errors) => {
                self.file_diagnostics.insert(id, (warnings, errors));
            }
        }

        Ok(())
    }

    /// Removes a source file's nodes and diagnostics from the in-memory store.
    ///
    /// Called when a source file is deleted from disk so its nodes are not
    /// written on the next `process` call.
    pub fn remove(&mut self, id: FileId) {
        if let Some(old_ids) = self.files.remove(&id) {
            for old_id in old_ids {
                self.nodes.remove(&old_id);
            }
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

    /// Writes all nodes to `output_dir/<node-id>.html`.
    ///
    /// Fatal I/O errors are returned as `Err`.
    pub fn process(&self, output_directory: &Path) -> anyhow::Result<()> {
        for (node_id, (html, _span)) in &self.nodes {
            let path = output_directory.join(format!("{node_id}.html"));
            std::fs::write(&path, html)?;
        }
        Ok(())
    }
}

/// Parses `document` into a map of node IDs to (HTML, span) pairs.
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
    node_exists: impl Fn(&str) -> bool,
) -> Result<HashMap<NodeId, (String, Span)>, EcoVec<SourceDiagnostic>> {
    let (spans, mut errors) = collect_node_spans(html_document);

    // Check for global duplicate identifiers before processing.
    errors.extend(
        spans
            .iter()
            .filter(|(id, _)| node_exists(id))
            .map(|(id, &span)| {
                SourceDiagnostic::error(span, eco_format!("duplicate node identifier: {id:?}"))
            }),
    );

    let mut nodes: HashMap<NodeId, (String, Span)> = HashMap::new();

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

        if transclude {
            subnode.replace_with_html(format!(
                r#"<wb-transclude identifier="{identifier}"></wb-transclude>"#
            ));
        } else {
            subnode.remove();
        }

        nodes.insert(identifier, (html, span));
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
                nodes.insert(identifier, (wb_node.html().to_string(), span));
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
) -> (HashMap<NodeId, Span>, EcoVec<SourceDiagnostic>) {
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
