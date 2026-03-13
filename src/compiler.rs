use std::collections::HashMap;
use std::path::Path;

use dom_query::Document;
use ecow::EcoVec;
use typst::World;
use typst::diag::{Severity, SourceDiagnostic, Warned};
use typst::syntax::FileId;
use typst_html::HtmlDocument;

const HTML_MESSAGE: &str = "html export is under active development and incomplete";

pub type NodeId = String;

pub struct Compiler {
    files: HashMap<FileId, Vec<NodeId>>,
    nodes: HashMap<NodeId, String>,
    file_diagnostics: HashMap<FileId, (EcoVec<SourceDiagnostic>, EcoVec<SourceDiagnostic>)>,
    // TODO: Definitely won't be just a Vec<String>, should map NodeIds to a a list of SourceDiagnostics
    // node_diagnostics: Vec<String>,
}

impl Compiler {
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
                    let document = Document::from(content.as_str());
                    let mut node_ids = Vec::new();

                    // Process subnodes deepest-first: reversed pre-order ensures a
                    // nested subnode is always processed before its parent subnode.
                    for subnode in document.select("wb-subnode").iter().rev() {
                        let Some(identifier) = subnode.attr("identifier") else {
                            continue;
                        };
                        let identifier = identifier.to_string();
                        let transclude = subnode
                            .attr("transclude")
                            .map(|v| v.as_ref() == "true")
                            .unwrap_or(true);

                        self.nodes
                            .insert(identifier.clone(), subnode.html().to_string());
                        node_ids.push(identifier.clone());

                        if transclude {
                            subnode.replace_with_html(format!(
                                r#"<wb-transclude identifier="{identifier}"></wb-transclude>"#
                            ));
                        } else {
                            subnode.remove();
                        }
                    }

                    // Extract the wb-node after subnodes have been replaced/removed.
                    if let Some(wb_node) = document.select("wb-node").iter().next()
                        && let Some(identifier) = wb_node.attr("identifier")
                    {
                        let identifier = identifier.to_string();
                        self.nodes
                            .insert(identifier.clone(), wb_node.html().to_string());
                        node_ids.push(identifier);
                    }

                    self.files.insert(id, node_ids);

                    if warnings.is_empty() {
                        self.file_diagnostics.remove(&id);
                    } else {
                        self.file_diagnostics.insert(id, (warnings, EcoVec::new()));
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

    pub fn file_diagnostics(
        &self,
    ) -> &HashMap<FileId, (EcoVec<SourceDiagnostic>, EcoVec<SourceDiagnostic>)> {
        &self.file_diagnostics
    }

    /// Writes all nodes to `output_dir/<node-id>.html`.
    ///
    /// Fatal I/O errors are returned as `Err`.
    pub fn process(&self, output_directory: &Path) -> anyhow::Result<()> {
        for (node_id, html) in &self.nodes {
            let path = output_directory.join(format!("{node_id}.html"));
            std::fs::write(&path, html)?;
        }
        Ok(())
    }

}
