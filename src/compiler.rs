use std::collections::HashMap;
use std::path::Path;

use dom_query::Document;
use ecow::EcoVec;
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

        let keep = |d: &mut SourceDiagnostic| {
            !(d.severity == Severity::Warning && d.message == HTML_MESSAGE)
        };

        warnings.retain(keep);

        match result {
            Ok(html_document) => match typst_html::html(&html_document) {
                Ok(content) => {
                    let spans = query_node_spans(&html_document);
                    let document = Document::from(content.as_str());
                    let mut nodes: HashMap<NodeId, (String, Span)> = HashMap::new();
                    let mut errors: EcoVec<SourceDiagnostic> = EcoVec::new();

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
                        let transclude = subnode
                            .attr("transclude")
                            .map(|v| v.as_ref() == "true")
                            .unwrap_or(true);

                        let span = spans.get(&identifier).copied().expect("bug: no span found for node identifier");
                        nodes.insert(identifier.clone(), (subnode.html().to_string(), span));

                        if transclude {
                            subnode.replace_with_html(format!(
                                r#"<wb-transclude identifier="{identifier}"></wb-transclude>"#
                            ));
                        } else {
                            subnode.remove();
                        }
                    }

                    // Extract the wb-node after subnodes have been replaced/removed.
                    if let Some(wb_node) = document.select("wb-node").iter().next() {
                        if let Some(identifier) = wb_node.attr("identifier") {
                            let identifier = identifier.to_string();
                            let span = spans.get(&identifier).copied().expect("bug: no span found for node identifier");
                            nodes.insert(identifier.clone(), (wb_node.html().to_string(), span));
                        } else {
                            errors.push(SourceDiagnostic::error(
                                Span::detached(),
                                "wb-node is missing an identifier",
                            ));
                        }
                    }

                    if errors.is_empty() {
                        self.files.insert(id, nodes.keys().cloned().collect());
                        self.nodes.extend(nodes);
                        if warnings.is_empty() {
                            self.file_diagnostics.remove(&id);
                        } else {
                            self.file_diagnostics.insert(id, (warnings, EcoVec::new()));
                        }
                    } else {
                        self.file_diagnostics.insert(id, (warnings, errors));
                    }
                }
                Err(mut errors) => {
                    errors.retain(keep);
                    self.file_diagnostics.insert(id, (warnings, errors));
                }
            },
            Err(mut errors) => {
                errors.retain(keep);
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

/// Walks `document`'s element tree once (iterative DFS) and returns a map
/// from each node identifier to the span of its `wb-node` or `wb-subnode`
/// element.
fn query_node_spans(document: &HtmlDocument) -> HashMap<String, Span> {
    let wb_node = HtmlTag::intern("wb-node").expect("wb-node is a valid tag");
    let wb_subnode = HtmlTag::intern("wb-subnode").expect("wb-subnode is a valid tag");
    let identifier = HtmlAttr::intern("identifier").expect("identifier is a valid attr");

    let mut spans = HashMap::new();
    let mut stack = vec![document.root()];

    while let Some(element) = stack.pop() {
        if (element.tag == wb_node || element.tag == wb_subnode)
            && let Some(id) = element.attrs.get(identifier)
        {
            spans.insert(id.to_string(), element.span);
        }
        for child in element.children.iter().rev() {
            if let HtmlNode::Element(child_elem) = child {
                stack.push(child_elem);
            }
        }
    }

    spans
}
