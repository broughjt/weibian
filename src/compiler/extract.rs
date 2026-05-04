use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use dom_query::{Document, Selection};
use ecow::{EcoVec, eco_format};
use typst::World;
use typst::diag::{Severity, SourceDiagnostic, Warned};
use typst::foundations::{Dict, NativeElement, Packed, Repr, Value};
use typst::introspection::{Introspector, MetadataElem};
use typst::syntax::Span;
use typst_html::{HtmlAttr, HtmlDocument, HtmlNode, HtmlTag};

use super::{Metadata, Node};

const HTML_MESSAGE: &str = "html export is under active development and incomplete";
const NO_WB_NODE: &str = "source file produced no wb-node";
const MULTIPLE_WB_NODES: &str = "source file produced multiple wb-node elements";
const WB_NODE_MISSING_IDENTIFIER: &str = "wb-node is missing an identifier";
const WB_SUBNODE_MISSING_IDENTIFIER: &str = "wb-subnode is missing an identifier";
const WB_NODE_MISSING_COUNTER: &str = "wb-node is missing a counter attribute";
const WB_SUBNODE_MISSING_COUNTER: &str = "wb-subnode is missing a counter attribute";
const WB_NODE_MISSING_TITLE: &str = "wb-node's first child must be a wb-title element";
const WB_SUBNODE_MISSING_TITLE: &str = "wb-subnode's first child must be a wb-title element";
const WB_SUBNODE_MISSING_TRANSCLUDE: &str = "wb-subnode is missing the transclude attribute";
const WB_TRANSCLUDE_MISSING_IDENTIFIER: &str = "wb-transclude is missing an identifier";
const WB_TRANSCLUDE_MISSING_COUNTER: &str = "wb-transclude is missing a counter attribute";
const LINK_MISSING_COUNTER: &str = "link is missing a data-counter attribute";
const BUG_NO_SPAN_FOR_COUNTER: &str = "bug: no span found for node counter";

// Rename entry to node everywhere

pub type NodeOutput = (String, Node<String>);

/// Compiles a source file and extracts its nodes.
pub fn compile<W: World>(world: &W) -> Warned<Result<Vec<NodeOutput>, EcoVec<SourceDiagnostic>>> {
    let Warned {
        output: result,
        mut warnings,
    } = typst::compile::<typst_html::HtmlDocument>(world);

    // Discard warnings about html being an unstable feature, html is kind
    // of the whole game here
    warnings.retain(|diagnostic: &mut SourceDiagnostic| {
        !(diagnostic.severity == Severity::Warning && diagnostic.message == HTML_MESSAGE)
    });

    let output = result.and_then(|html_document| {
        typst_html::html(&html_document).and_then(|html| {
            let (spans, span_errors) = collect_node_spans(&html_document);
            let (node, transclusion, link, meta_errors) =
                collect_metadata(html_document.introspector().as_ref(), &spans);
            let mut errors = span_errors;
            errors.extend(meta_errors);

            if errors.is_empty() {
                let file_output = FileOutput {
                    html,
                    spans,
                    metadata: MetadataMaps {
                        node,
                        transclusion,
                        link,
                    },
                };

                extract(file_output)
            } else {
                Err(errors)
            }
        })
    });

    Warned { output, warnings }
}

#[derive(Clone, Debug)]
struct FileOutput {
    pub html: String,
    pub spans: HashMap<u32, Span>,
    pub metadata: MetadataMaps,
}

#[derive(Clone, Debug)]
struct MetadataMaps {
    node: HashMap<u32, Metadata>,
    transclusion: HashMap<u32, Metadata>,
    link: HashMap<u32, Metadata>,
}

/// Parses the HTML in `output` into extracted node occurrences.
///
/// Returns `Err` with all collected diagnostics if any validation errors occur,
/// or `Ok` with the node vector on success.
fn extract(output: FileOutput) -> Result<Vec<NodeOutput>, EcoVec<SourceDiagnostic>> {
    let FileOutput {
        html,
        spans,
        mut metadata,
    } = output;
    let mut errors = EcoVec::new();
    let document = Document::from(html);
    let mut nodes = Vec::with_capacity(spans.len());
    let mut seen_node_counters = HashSet::with_capacity(spans.len());
    let mut synthetic_counter: u32 = document
        .select("wb-transclude")
        .iter()
        .filter_map(|e| e.attr("counter")?.parse::<u32>().ok())
        .chain(metadata.transclusion.keys().copied())
        .max()
        .map_or(0, |n| {
            n.checked_add(1).expect("transclusion counter overflow")
        });

    // Process subnodes deepest-first: reversed pre-order ensures a
    // nested subnode is always processed before its parent subnode.
    for subnode in document.select("wb-subnode").iter().rev() {
        let Some((node_counter, (identifier, entry))) = extract_node_content(
            &subnode,
            true,
            &spans,
            &mut metadata,
            &mut seen_node_counters,
            &mut errors,
        ) else {
            continue;
        };
        let transclude = match subnode.attr("transclude").as_deref() {
            Some("true") => true,
            Some("false") => false,
            Some(other) => {
                errors.push(invalid_transclude_value_diagnostic(other));
                continue;
            }
            None => {
                errors.push(SourceDiagnostic::error(
                    entry.span,
                    WB_SUBNODE_MISSING_TRANSCLUDE,
                ));
                continue;
            }
        };

        if transclude {
            let counter = synthetic_counter;
            synthetic_counter = synthetic_counter
                .checked_add(1)
                .expect("transclusion counter overflow");

            if !entry.node_metadata.is_empty() {
                metadata
                    .transclusion
                    .insert(counter, entry.node_metadata.clone());
            }
            subnode.replace_with_html(format!(
                r#"<wb-transclude identifier="{identifier}" counter="{counter}"></wb-transclude>"#
            ));
        } else {
            subnode.remove();
        }

        nodes.push((node_counter, (identifier, entry)));
    }

    // Extract the wb-node after subnodes have been replaced/removed.
    let mut node_iter = document.select("wb-node").iter();

    match node_iter.next() {
        None => {
            errors.push(SourceDiagnostic::error(Span::detached(), NO_WB_NODE));
        }
        Some(wb_node) => {
            if let Some((node_counter, output)) = extract_node_content(
                &wb_node,
                false,
                &spans,
                &mut metadata,
                &mut seen_node_counters,
                &mut errors,
            ) {
                nodes.push((node_counter, output));
            }

            errors.extend(node_iter.map(|extra| {
                let span = extra
                    .attr("counter")
                    .and_then(|counter| counter.parse::<u32>().ok())
                    .map(|counter| {
                        spans
                            .get(&counter)
                            .copied()
                            .expect("bug: no span found for wb-node counter")
                    })
                    .unwrap_or(Span::detached());

                SourceDiagnostic::error(span, MULTIPLE_WB_NODES)
            }));
        }
    }

    errors.extend(
        metadata
            .node
            .keys()
            .copied()
            .map(orphaned_node_metadata_diagnostic),
    );
    errors.extend(
        metadata
            .transclusion
            .keys()
            .copied()
            .map(orphaned_transclusion_metadata_diagnostic),
    );
    errors.extend(
        metadata
            .link
            .keys()
            .copied()
            .map(orphaned_link_metadata_diagnostic),
    );

    if errors.is_empty() {
        nodes.sort_by_key(|(counter, _)| *counter);

        Ok(nodes.into_iter().map(|(_, output)| output).collect())
    } else {
        Err(errors)
    }
}

/// Extracts the content of a `wb-node` or `wb-subnode` element into a
/// [`NodeOutput`], collecting its transclusions and links and consuming its
/// metadata from the provided maps.
///
/// Returns `None` (pushing an error) if the identifier attribute is missing or
/// if the element's first child is not a `wb-title` element.
fn extract_node_content(
    element: &Selection,
    is_subnode: bool,
    spans: &HashMap<u32, Span>,
    metadata: &mut MetadataMaps,
    seen_node_counters: &mut HashSet<u32>,
    errors: &mut EcoVec<SourceDiagnostic>,
) -> Option<(u32, NodeOutput)> {
    let (transclusions, transclusion_metadata) = collect_transclusions(element, metadata, errors);
    let (links, link_metadata) = collect_links(element, metadata, errors);

    let counter = match element.attr("counter").as_deref() {
        Some(n) => match n.parse::<u32>() {
            Ok(n) => Some(n),
            Err(_) => {
                errors.push(invalid_node_counter_diagnostic(
                    Span::detached(),
                    is_subnode,
                    n,
                ));
                None
            }
        },
        None => {
            errors.push(missing_node_counter_diagnostic(
                Span::detached(),
                is_subnode,
            ));
            None
        }
    }?;
    if !seen_node_counters.insert(counter) {
        errors.push(duplicate_node_counter_diagnostic(counter));
        return None;
    }
    let span = spans.get(&counter).copied().expect(BUG_NO_SPAN_FOR_COUNTER);
    // TODO: Fix if this doesn't work.
    let file_id = span.id().expect("Span should have FileId");

    let node_metadata = metadata.node.remove(&counter).unwrap_or_default();

    let Some(identifier) = element.attr("identifier") else {
        errors.push(missing_identifier_diagnostic(is_subnode));
        return None;
    };
    let identifier = identifier.to_string();

    let title_selection = element.children().first();
    if !title_selection
        .nodes()
        .first()
        .is_some_and(|n| n.has_name("wb-title"))
    {
        errors.push(missing_title_diagnostic(is_subnode));
        return None;
    }
    let title = title_selection.inner_html().to_string();
    let title_text = title_selection.text().to_string();
    title_selection.remove();

    let body_html = element.inner_html().to_string();

    Some((
        counter,
        (
            identifier,
            Node {
                body_html,
                title,
                title_text,
                file_id,
                span,
                node_metadata,
                transclusions,
                transclusion_metadata,
                links,
                link_metadata,
            },
        ),
    ))
}

/// Collects transclusion targets and their metadata from `wb-transclude`
/// elements within `element`.
fn collect_transclusions(
    element: &Selection,
    metadata: &mut MetadataMaps,
    errors: &mut EcoVec<SourceDiagnostic>,
) -> (EcoVec<String>, HashMap<u32, Metadata>) {
    let mut targets = EcoVec::new();
    let mut node_metadata = HashMap::new();
    for wb_transclude in element.select("wb-transclude").iter() {
        let id = match wb_transclude.attr("identifier").as_deref() {
            Some(id) => id.to_owned(),
            None => {
                errors.push(SourceDiagnostic::error(
                    Span::detached(),
                    WB_TRANSCLUDE_MISSING_IDENTIFIER,
                ));
                continue;
            }
        };
        let counter = match wb_transclude.attr("counter").as_deref() {
            Some(n) => match n.parse::<u32>() {
                Ok(n) => n,
                Err(_) => {
                    errors.push(invalid_transclude_counter_diagnostic(n));
                    continue;
                }
            },
            None => {
                errors.push(SourceDiagnostic::error(
                    Span::detached(),
                    WB_TRANSCLUDE_MISSING_COUNTER,
                ));
                continue;
            }
        };

        if let Some(meta) = metadata.transclusion.remove(&counter) {
            node_metadata.insert(counter, meta);
        }
        targets.push(id);
    }
    (targets, node_metadata)
}

/// Collects link targets and their metadata from `<a href="wb:...">` elements
/// within `element`.
fn collect_links(
    element: &Selection,
    metadata: &mut MetadataMaps,
    errors: &mut EcoVec<SourceDiagnostic>,
) -> (EcoVec<String>, HashMap<u32, Metadata>) {
    let mut targets = EcoVec::new();
    let mut node_metadata = HashMap::new();
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
                    errors.push(invalid_link_counter_diagnostic(n));
                    continue;
                }
            },
            None => {
                errors.push(SourceDiagnostic::error(
                    Span::detached(),
                    LINK_MISSING_COUNTER,
                ));
                continue;
            }
        };

        if let Some(meta) = metadata.link.remove(&counter) {
            node_metadata.insert(counter, meta);
        }
        targets.push(id);
    }
    (targets, node_metadata)
}

/// Queries the introspector for `#metadata(...)` elements that carry node or
/// transclusion call-site metadata, and returns them as two separate maps.
///
/// Metadata elements are identified by a `wb-metadata` key whose value is a
/// two-element array `[kind, discriminant]`:
/// - `["node", counter]`         — node/subnode metadata, keyed by node counter integer
/// - `["transclude", counter]`   — transclusion call-site metadata, keyed by counter integer
/// - `["link", counter]`         — link call-site metadata, keyed by counter integer
///
/// Errors are pushed for:
/// - `wb-metadata` present but not a two-element array of the expected shape
/// - node counter not present in `spans` (unknown node)
/// - duplicate entries for the same node or counter
#[allow(clippy::type_complexity)]
fn collect_metadata<I: Introspector>(
    introspector: &I,
    spans: &HashMap<u32, Span>,
) -> (
    HashMap<u32, Metadata>,
    HashMap<u32, Metadata>,
    HashMap<u32, Metadata>,
    EcoVec<SourceDiagnostic>,
) {
    let selector = MetadataElem::ELEM.select();
    let items = introspector.query(&selector);
    let mut errors = EcoVec::new();
    let mut node_result: HashMap<u32, Metadata> = HashMap::new();
    let mut transclusion_result: HashMap<u32, Metadata> = HashMap::new();
    let mut link_result: HashMap<u32, Metadata> = HashMap::new();

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
                    let Value::Int(counter_i64) = discriminant else {
                        errors.push(SourceDiagnostic::error(
                            item.span(),
                            "\"wb-metadata\" node counter must be an integer",
                        ));
                        continue;
                    };
                    let counter = match u32::try_from(*counter_i64) {
                        Ok(n) => n,
                        Err(_) => {
                            errors.push(SourceDiagnostic::error(
                                item.span(),
                                eco_format!(
                                    "\"wb-metadata\" node counter out of range: {counter_i64}"
                                ),
                            ));
                            continue;
                        }
                    };

                    if !spans.contains_key(&counter) {
                        errors.push(SourceDiagnostic::error(
                            item.span(),
                            eco_format!("metadata for unknown node counter: {counter}"),
                        ));
                        continue;
                    }

                    match node_result.entry(counter) {
                        Entry::Vacant(entry) => {
                            entry.insert(normalize_metadata(dictionary));
                        }
                        Entry::Occupied(e) => {
                            errors.push(SourceDiagnostic::error(
                                item.span(),
                                eco_format!("duplicate metadata for node counter: {}", e.key()),
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
/// each node counter to the span of its `wb-node` or `wb-subnode` element.
fn collect_node_spans(document: &HtmlDocument) -> (HashMap<u32, Span>, EcoVec<SourceDiagnostic>) {
    let wb_node = HtmlTag::intern("wb-node").expect("wb-node is a valid tag");
    let wb_subnode = HtmlTag::intern("wb-subnode").expect("wb-subnode is a valid tag");
    let counter_attr = HtmlAttr::intern("counter").expect("counter is a valid attr");

    let mut spans = HashMap::new();
    let mut errors = EcoVec::new();
    let mut stack = vec![document.root()];

    while let Some(element) = stack.pop() {
        if element.tag == wb_node || element.tag == wb_subnode {
            let Some(counter_string) = element.attrs.get(counter_attr) else {
                errors.push(missing_node_counter_diagnostic(
                    element.span,
                    element.tag == wb_subnode,
                ));
                continue;
            };
            let counter = match counter_string.parse::<u32>() {
                Ok(counter) => counter,
                Err(_) => {
                    errors.push(invalid_node_counter_diagnostic(
                        element.span,
                        element.tag == wb_subnode,
                        counter_string.as_str(),
                    ));
                    continue;
                }
            };

            match spans.entry(counter) {
                Entry::Occupied(_) => errors.push(duplicate_node_counter_diagnostic(counter)),
                Entry::Vacant(entry) => {
                    entry.insert(element.span);
                }
            }
        }

        stack.extend(
            element
                .children
                .iter()
                .rev()
                .filter_map(|child| match child {
                    HtmlNode::Element(element) => Some(element),
                    _ => None,
                }),
        );
    }

    (spans, errors)
}

fn missing_identifier_diagnostic(is_subnode: bool) -> SourceDiagnostic {
    SourceDiagnostic::error(
        Span::detached(),
        if is_subnode {
            WB_SUBNODE_MISSING_IDENTIFIER
        } else {
            WB_NODE_MISSING_IDENTIFIER
        },
    )
}

fn missing_title_diagnostic(is_subnode: bool) -> SourceDiagnostic {
    SourceDiagnostic::error(
        Span::detached(),
        if is_subnode {
            WB_SUBNODE_MISSING_TITLE
        } else {
            WB_NODE_MISSING_TITLE
        },
    )
}

fn missing_node_counter_diagnostic(span: Span, is_subnode: bool) -> SourceDiagnostic {
    SourceDiagnostic::error(
        span,
        if is_subnode {
            WB_SUBNODE_MISSING_COUNTER
        } else {
            WB_NODE_MISSING_COUNTER
        },
    )
}

fn invalid_node_counter_diagnostic(span: Span, is_subnode: bool, value: &str) -> SourceDiagnostic {
    SourceDiagnostic::error(
        span,
        if is_subnode {
            eco_format!("wb-subnode has invalid counter: {value:?}")
        } else {
            eco_format!("wb-node has invalid counter: {value:?}")
        },
    )
}

fn duplicate_node_counter_diagnostic(counter: u32) -> SourceDiagnostic {
    SourceDiagnostic::error(
        Span::detached(),
        eco_format!("duplicate node counter: {counter}"),
    )
}

fn invalid_transclude_value_diagnostic(value: &str) -> SourceDiagnostic {
    SourceDiagnostic::error(
        Span::detached(),
        eco_format!("wb-subnode has invalid transclude value: {value:?}"),
    )
}

fn invalid_transclude_counter_diagnostic(value: &str) -> SourceDiagnostic {
    SourceDiagnostic::error(
        Span::detached(),
        eco_format!("wb-transclude has invalid counter: {value:?}"),
    )
}

fn invalid_link_counter_diagnostic(value: &str) -> SourceDiagnostic {
    SourceDiagnostic::error(
        Span::detached(),
        eco_format!("link has invalid data-counter: {value:?}"),
    )
}

fn orphaned_node_metadata_diagnostic(counter: u32) -> SourceDiagnostic {
    SourceDiagnostic::error(
        Span::detached(),
        eco_format!("node metadata for counter {counter} has no corresponding node element"),
    )
}

fn orphaned_transclusion_metadata_diagnostic(counter: u32) -> SourceDiagnostic {
    SourceDiagnostic::error(
        Span::detached(),
        eco_format!(
            "transclusion metadata for counter {counter} has no corresponding wb-transclude element"
        ),
    )
}

fn orphaned_link_metadata_diagnostic(counter: u32) -> SourceDiagnostic {
    SourceDiagnostic::error(
        Span::detached(),
        eco_format!("link metadata for counter {counter} has no corresponding link element"),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fmt::Write;

    use dom_query::Document;
    use proptest::prelude::*;
    use typst::syntax::Span;

    use typst::diag::SourceDiagnostic;

    use super::*;
    use crate::compiler::Metadata;

    fn transclude_counters(html: &str) -> Vec<String> {
        Document::from(html)
            .select("wb-transclude[counter]")
            .iter()
            .filter_map(|e| e.attr("counter").map(|c| c.to_string()))
            .collect()
    }

    fn link_counters(html: &str) -> Vec<String> {
        Document::from(html)
            .select("a[data-counter]")
            .iter()
            .filter_map(|e| e.attr("data-counter").map(|c| c.to_string()))
            .collect()
    }

    fn check_body_html(node: &MockNode, body_html: &str) -> Result<(), TestCaseError> {
        let document = Document::from(body_html);
        for element in &node.body {
            match element {
                MockElement::Text(t) => {
                    prop_assert!(
                        body_html.contains(t.as_str()),
                        "text {t:?} not found in body_html: {body_html:?}",
                    );
                }
                MockElement::Link(l) => {
                    let selector = format!(r#"a[href="wb:{}"]"#, l.target);
                    prop_assert!(
                        document.select(&selector).exists(),
                        "link to {:?} not found in body_html: {body_html:?}",
                        l.target,
                    );
                }
                MockElement::Transclusion(t) => {
                    let selector = format!(r#"wb-transclude[identifier="{}"]"#, t.target);
                    prop_assert!(
                        document.select(&selector).exists(),
                        "transclude of {:?} not found in body_html: {body_html:?}",
                        t.target,
                    );
                }
                MockElement::Subnode(s) if s.transclude => {
                    let selector = format!(r#"wb-transclude[identifier="{}"]"#, s.node.identifier);
                    prop_assert!(
                        document.select(&selector).exists(),
                        "synthetic transclude for {:?} not found in body_html: {body_html:?}",
                        s.node.identifier,
                    );
                }
                MockElement::Subnode(s) => {
                    let selector = format!(r#"wb-subnode[identifier="{}"]"#, s.node.identifier);
                    prop_assert!(
                        !document.select(&selector).exists(),
                        "non-transcluded subnode {:?} found in body_html: {body_html:?}",
                        s.node.identifier,
                    );
                }
            }
        }
        Ok(())
    }

    const METADATA_VEC_COUNT_MAX: usize = 10;
    const METADATA_ENTRIES_COUNT_MAX: usize = 16;
    const BODY_ELEMENTS_MAX: usize = 16;
    const BODY_DEPTH: u32 = 4;
    const BODY_DESIRED_SIZE: u32 = 32;
    const BODY_BRANCH_SIZE: u32 = 8;

    #[derive(Debug, Clone)]
    struct MockFile {
        primary: MockNode,
    }

    #[derive(Debug, Clone)]
    struct MockNode {
        counter: u32,
        identifier: String,
        title: String,
        metadata: Option<Metadata>,
        body: Vec<MockElement>,
    }

    #[derive(Debug, Clone)]
    enum MockElement {
        Text(String),
        Link(MockLink),
        Transclusion(MockTransclusion),
        Subnode(MockSubnode),
    }

    #[derive(Debug, Clone)]
    struct MockSubnode {
        node: MockNode,
        transclude: bool,
    }

    #[derive(Debug, Clone)]
    struct MockLink {
        target: String,
        content: Option<String>,
        metadata: Option<Metadata>,
    }

    #[derive(Debug, Clone)]
    struct MockTransclusion {
        target: String,
        metadata: Option<Metadata>,
    }

    #[derive(Debug)]
    struct ExpectedOutput {
        counter: u32,
        identifier: String,
        title: String,
        node_metadata: Metadata,
        transclusions: Vec<String>,
        links: Vec<String>,
        transclusion_metadata: HashMap<u32, Metadata>,
        link_metadata: HashMap<u32, Metadata>,
    }

    impl MockFile {
        fn walk(&self, mut f: impl FnMut(&MockNode, bool)) {
            let mut stack: Vec<(&MockNode, bool)> = vec![(&self.primary, false)];
            while let Some((node, is_subnode)) = stack.pop() {
                f(node, is_subnode);
                for element in node.body.iter().rev() {
                    if let MockElement::Subnode(subnode) = element {
                        stack.push((&subnode.node, true));
                    }
                }
            }
        }

        fn walk_mut(&mut self, mut f: impl FnMut(&mut MockNode, bool)) {
            let mut stack: Vec<(&mut MockNode, bool)> = vec![(&mut self.primary, false)];
            while let Some((node, is_subnode)) = stack.pop() {
                f(node, is_subnode);
                for element in node.body.iter_mut().rev() {
                    if let MockElement::Subnode(subnode) = element {
                        stack.push((&mut subnode.node, true));
                    }
                }
            }
        }

        fn render(&self) -> (FileOutput, HashMap<u32, ExpectedOutput>) {
            let mut html = String::new();
            let mut spans = HashMap::new();
            let mut file_node_metadata = HashMap::new();
            let mut file_transclusion_metadata = HashMap::new();
            let mut file_link_metadata = HashMap::new();
            let mut synthetic_transclusions: Vec<(u32, Option<Metadata>)> = Vec::new();
            let mut transclusion_counter = 0u32;
            let mut link_counter = 0u32;
            let mut expected = HashMap::new();

            enum Work<'a> {
                /// Open a node: write its opening tag + title, then push its
                /// body elements (reversed) followed by a Close.
                Open {
                    node: &'a MockNode,
                    tag: &'static str,
                    option_transclude: Option<bool>,
                },
                /// Process a single leaf body element.
                Element(&'a MockElement),
                /// Close the current node: write the closing tag, register
                /// span/metadata, finalize the expected output.
                Close {
                    counter: u32,
                    identifier: &'a str,
                    tag: &'static str,
                    title: &'a str,
                    node_metadata: &'a Option<Metadata>,
                },
            }

            struct EdgeInfo {
                owner_counter: u32,
                transclusions: Vec<String>,
                links: Vec<String>,
                transclusion_metadata: HashMap<u32, Metadata>,
                link_metadata: HashMap<u32, Metadata>,
            }

            let mut stack: Vec<Work> = vec![Work::Open {
                node: &self.primary,
                tag: "wb-node",
                option_transclude: None,
            }];
            let mut edge_stack: Vec<EdgeInfo> = Vec::new();

            while let Some(work) = stack.pop() {
                match work {
                    Work::Open {
                        node,
                        tag,
                        option_transclude,
                    } => {
                        write!(
                            html,
                            r#"<{tag} identifier="{}" counter="{}""#,
                            node.identifier, node.counter
                        )
                        .unwrap();
                        if let Some(transclude) = option_transclude {
                            let value = if transclude { "true" } else { "false" };
                            write!(html, r#" transclude="{value}""#).unwrap();
                        }
                        html.push('>');
                        write!(html, "<wb-title>{}</wb-title>", node.title).unwrap();

                        stack.push(Work::Close {
                            counter: node.counter,
                            identifier: &node.identifier,
                            tag,
                            title: &node.title,
                            node_metadata: &node.metadata,
                        });
                        stack.extend(node.body.iter().rev().map(Work::Element));

                        edge_stack.push(EdgeInfo {
                            owner_counter: node.counter,
                            transclusions: Vec::new(),
                            links: Vec::new(),
                            transclusion_metadata: HashMap::new(),
                            link_metadata: HashMap::new(),
                        });
                    }
                    Work::Element(element) => {
                        let edges = edge_stack.last_mut().unwrap();

                        match element {
                            MockElement::Text(text) => {
                                write!(html, "<p>{text}</p>").unwrap();
                            }
                            MockElement::Link(link) => {
                                let counter = link_counter;
                                link_counter += 1;
                                let content = link.content.as_deref().unwrap_or_default();
                                write!(
                                    html,
                                    r#"<a href="wb:{}" data-counter="{counter}">{content}</a>"#,
                                    link.target,
                                )
                                .unwrap();
                                if let Some(metadata) = &link.metadata
                                    && !metadata.is_empty()
                                {
                                    assert!(
                                        file_link_metadata
                                            .insert(counter, metadata.clone())
                                            .is_none(),
                                        "duplicate link metadata: {counter}",
                                    );
                                    assert!(
                                        edges
                                            .link_metadata
                                            .insert(counter, metadata.clone())
                                            .is_none(),
                                        "duplicate expected link metadata: {counter}",
                                    );
                                }
                                edges.links.push(link.target.clone());
                            }
                            MockElement::Transclusion(t) => {
                                let counter = transclusion_counter;
                                transclusion_counter += 1;
                                write!(
                                    html,
                                    r#"<wb-transclude identifier="{}" counter="{counter}"></wb-transclude>"#,
                                    t.target,
                                )
                                .unwrap();
                                if let Some(metadata) = &t.metadata
                                    && !metadata.is_empty()
                                {
                                    assert!(
                                        file_transclusion_metadata
                                            .insert(counter, metadata.clone())
                                            .is_none(),
                                        "duplicate transclusion metadata: {counter}",
                                    );
                                    assert!(
                                        edges
                                            .transclusion_metadata
                                            .insert(counter, metadata.clone())
                                            .is_none(),
                                        "duplicate expected transclusion metadata: {counter}",
                                    );
                                }
                                edges.transclusions.push(t.target.clone());
                            }
                            MockElement::Subnode(subnode) => {
                                if subnode.transclude {
                                    edges.transclusions.push(subnode.node.identifier.clone());
                                    synthetic_transclusions
                                        .push((edges.owner_counter, subnode.node.metadata.clone()));
                                }
                                // Push the subnode as a new `Open` work item. It
                                // will be fully processed before we we continue
                                // with the current node's remaining elements.
                                stack.push(Work::Open {
                                    node: &subnode.node,
                                    tag: "wb-subnode",
                                    option_transclude: Some(subnode.transclude),
                                });
                            }
                        }
                    }
                    Work::Close {
                        counter,
                        identifier,
                        tag,
                        title,
                        node_metadata,
                    } => {
                        write!(html, "</{tag}>").unwrap();

                        assert!(
                            spans.insert(counter, Span::detached()).is_none(),
                            "duplicate span: {counter}",
                        );
                        if let Some(metadata) = node_metadata
                            && !metadata.is_empty()
                        {
                            assert!(
                                file_node_metadata
                                    .insert(counter, metadata.clone())
                                    .is_none(),
                                "duplicate node metadata: {counter}",
                            );
                        }

                        let edges = edge_stack.pop().unwrap();
                        expected.insert(
                            counter,
                            ExpectedOutput {
                                counter,
                                identifier: identifier.to_owned(),
                                title: title.to_owned(),
                                node_metadata: node_metadata.clone().unwrap_or_default(),
                                transclusions: edges.transclusions,
                                links: edges.links,
                                transclusion_metadata: edges.transclusion_metadata,
                                link_metadata: edges.link_metadata,
                            },
                        );
                    }
                }
            }

            let mut synthetic_counter = transclusion_counter;
            for (owner_counter, metadata) in synthetic_transclusions.into_iter().rev() {
                let counter = synthetic_counter;
                synthetic_counter = synthetic_counter
                    .checked_add(1)
                    .expect("synthetic transclusion counter overflow");

                if let Some(metadata) = metadata
                    && !metadata.is_empty()
                {
                    assert!(
                        expected
                            .get_mut(&owner_counter)
                            .expect("bug: missing expected owner for synthetic transclusion")
                            .transclusion_metadata
                            .insert(counter, metadata)
                            .is_none(),
                        "duplicate expected synthetic transclusion metadata: {counter}",
                    );
                }
            }

            let file_output = FileOutput {
                html,
                spans,
                metadata: MetadataMaps {
                    node: file_node_metadata,
                    transclusion: file_transclusion_metadata,
                    link: file_link_metadata,
                },
            };

            (file_output, expected)
        }
    }

    fn metadata_strategy() -> impl Strategy<Value = Metadata> {
        proptest::collection::hash_map(
            "[a-z]+",
            proptest::collection::vec("[a-z0-9]+", 0..=METADATA_VEC_COUNT_MAX),
            0..=METADATA_ENTRIES_COUNT_MAX,
        )
    }

    fn leaf_element_strategy() -> impl Strategy<Value = MockElement> {
        prop_oneof![
            "[a-z ]{1,15}".prop_map(MockElement::Text),
            (
                "[a-z][a-z0-9]{0,7}",
                proptest::option::of("[a-z ]{1,10}"),
                proptest::option::of(metadata_strategy()),
            )
                .prop_map(|(target, content, metadata)| MockElement::Link(MockLink {
                    target,
                    content,
                    metadata,
                })),
            (
                "[a-z][a-z0-9]{0,7}",
                proptest::option::of(metadata_strategy())
            )
                .prop_map(|(target, metadata)| {
                    MockElement::Transclusion(MockTransclusion { target, metadata })
                }),
        ]
    }

    fn node_strategy(
        body: impl Strategy<Value = Vec<MockElement>>,
    ) -> impl Strategy<Value = MockNode> {
        (
            "[A-Za-z ]{0,12}",
            proptest::option::of(metadata_strategy()),
            body,
        )
            .prop_map(|(title, metadata, body)| MockNode {
                counter: 0,
                identifier: String::new(),
                title,
                metadata,
                body,
            })
    }

    fn element_strategy() -> impl Strategy<Value = MockElement> {
        leaf_element_strategy().prop_recursive(
            BODY_DEPTH,
            BODY_DESIRED_SIZE,
            BODY_BRANCH_SIZE,
            |inner| {
                prop_oneof![
                    3 => leaf_element_strategy(),
                    1 => (
                        node_strategy(proptest::collection::vec(inner, 0..=BODY_ELEMENTS_MAX)),
                        proptest::bool::ANY,
                    )
                        .prop_map(|(node, transclude)| MockElement::Subnode(MockSubnode {
                            node,
                            transclude,
                        })),
                ]
            },
        )
    }

    fn mock_file_strategy() -> impl Strategy<Value = MockFile> {
        node_strategy(proptest::collection::vec(
            element_strategy(),
            0..=BODY_ELEMENTS_MAX,
        ))
        .prop_map(|primary| {
            let mut file = MockFile { primary };
            let mut next_id = 0u32;
            file.walk_mut(|node, _| {
                node.counter = next_id;
                node.identifier = format!("n{next_id}");
                next_id = next_id
                    .checked_add(1)
                    .expect("node identifier counter overflow");
            });
            file
        })
    }

    #[test]
    fn missing_wb_node() {
        let output = FileOutput {
            html: String::new(),
            spans: HashMap::new(),
            metadata: MetadataMaps {
                node: HashMap::new(),
                transclusion: HashMap::new(),
                link: HashMap::new(),
            },
        };
        let errors = extract(output).unwrap_err();
        assert!(errors.iter().any(|e| e.message == NO_WB_NODE));
    }

    #[test]
    fn multiple_wb_nodes() {
        let output = FileOutput {
            html: concat!(
                r#"<wb-node identifier="n0" counter="0"><wb-title>A</wb-title></wb-node>"#,
                r#"<wb-node identifier="n1" counter="1"><wb-title>B</wb-title></wb-node>"#,
            )
            .to_owned(),
            spans: HashMap::from([(0, Span::detached()), (1, Span::detached())]),
            metadata: MetadataMaps {
                node: HashMap::new(),
                transclusion: HashMap::new(),
                link: HashMap::new(),
            },
        };
        let errors = extract(output).unwrap_err();
        assert!(errors.iter().any(|e| e.message == MULTIPLE_WB_NODES));
    }

    #[test]
    fn missing_node_counter() {
        let output = FileOutput {
            html: r#"<wb-node identifier="n0"><wb-title>A</wb-title></wb-node>"#.to_owned(),
            spans: HashMap::new(),
            metadata: MetadataMaps {
                node: HashMap::new(),
                transclusion: HashMap::new(),
                link: HashMap::new(),
            },
        };
        let errors = extract(output).unwrap_err();
        assert!(errors.iter().any(|e| e.message == WB_NODE_MISSING_COUNTER));
    }

    #[test]
    fn invalid_node_counter() {
        let output = FileOutput {
            html: r#"<wb-node identifier="n0" counter="nan"><wb-title>A</wb-title></wb-node>"#
                .to_owned(),
            spans: HashMap::new(),
            metadata: MetadataMaps {
                node: HashMap::new(),
                transclusion: HashMap::new(),
                link: HashMap::new(),
            },
        };
        let errors = extract(output).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e == &invalid_node_counter_diagnostic(Span::detached(), false, "nan"))
        );
    }

    #[test]
    fn duplicate_node_counter() {
        let output = FileOutput {
            html: concat!(
                r#"<wb-node identifier="n0" counter="0"><wb-title>A</wb-title>"#,
                r#"<wb-subnode identifier="n1" counter="0" transclude="false"><wb-title>B</wb-title></wb-subnode>"#,
                r#"</wb-node>"#,
            )
            .to_owned(),
            spans: HashMap::from([(0, Span::detached())]),
            metadata: MetadataMaps {
                node: HashMap::new(),
                transclusion: HashMap::new(),
                link: HashMap::new(),
            },
        };
        let errors = extract(output).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e == &duplicate_node_counter_diagnostic(0))
        );
    }

    #[test]
    fn orphaned_node_metadata_counter() {
        let output = FileOutput {
            html: r#"<wb-node identifier="n0" counter="0"><wb-title>A</wb-title></wb-node>"#
                .to_owned(),
            spans: HashMap::from([(0, Span::detached())]),
            metadata: MetadataMaps {
                node: HashMap::from([(1, HashMap::from([("k".to_owned(), vec!["v".to_owned()])]))]),
                transclusion: HashMap::new(),
                link: HashMap::new(),
            },
        };
        let errors = extract(output).unwrap_err();
        assert_eq!(errors.to_vec(), vec![orphaned_node_metadata_diagnostic(1)]);
    }

    proptest! {
        #[test]
        fn happy_case(mut file in mock_file_strategy()) {
            let (file_output, expected_nodes) = file.render();
            let actual_nodes = extract(file_output).unwrap();

            prop_assert_eq!(actual_nodes.len(), expected_nodes.len());
            let mut expected_nodes: Vec<&ExpectedOutput> = expected_nodes.values().collect();
            expected_nodes.sort_by_key(|node| node.counter);
            prop_assert_eq!(
                actual_nodes
                    .iter()
                    .map(|(identifier, _)| identifier.as_str())
                    .collect::<Vec<_>>(),
                expected_nodes
                    .iter()
                    .map(|node| node.identifier.as_str())
                    .collect::<Vec<_>>(),
            );

            let mut counter_to_node: HashMap<u32, MockNode> = HashMap::new();
            file.walk_mut(|node, _is_subnode| { counter_to_node.insert(node.counter, node.clone()); });

            for (actual, expected) in actual_nodes.iter().zip(expected_nodes) {

                prop_assert_eq!(&actual.0, &expected.identifier);
                prop_assert_eq!(&actual.1.title, &expected.title);
                prop_assert_eq!(&actual.1.title_text, &expected.title);
                prop_assert_eq!(&actual.1.node_metadata, &expected.node_metadata);
                prop_assert_eq!(&actual.1.transclusions, &expected.transclusions);
                prop_assert_eq!(&actual.1.links, &expected.links);
                prop_assert_eq!(&actual.1.transclusion_metadata, &expected.transclusion_metadata);
                prop_assert_eq!(&actual.1.link_metadata, &expected.link_metadata);

                prop_assert!(actual.1.transclusion_metadata.values().all(|m| !m.is_empty()));
                prop_assert!(actual.1.link_metadata.values().all(|m| !m.is_empty()));

                let document = Document::from(actual.1.body_html.as_str());

                prop_assert!(document.select("wb-subnode").iter().next().is_none());
                prop_assert!(document.select("wb-title").iter().next().is_none());

                check_body_html(&counter_to_node[&expected.counter], &actual.1.body_html)?;
            }
        }

        #[test]
        fn missing_node_identifier(
            (file, ids) in mock_file_strategy()
                .prop_flat_map(|file| {
                    let mut ids = Vec::new();
                    file.walk(|node, is_subnode| ids.push((node.identifier.clone(), is_subnode)));
                    let n = ids.len();
                    proptest::sample::subsequence(ids, 1..=n)
                        .prop_map(move |stripped| (file.clone(), stripped))
                })
        ) {
            let (mut output, _) = file.render();

            let document = Document::from(output.html.as_str());
            for (id, _) in &ids {
                let selector = format!(r#"wb-node[identifier="{id}"], wb-subnode[identifier="{id}"]"#);
                document.select(&selector).remove_attr("identifier");
            }
            output.html = document.html().to_string();

            let mut expected: Vec<SourceDiagnostic> = ids
                .iter()
                .map(|(_, is_subnode)| missing_identifier_diagnostic(*is_subnode))
                .collect();
            let mut actual = extract(output).unwrap_err().to_vec();

            actual.sort_by(|a, b| a.message.cmp(&b.message));
            expected.sort_by(|a, b| a.message.cmp(&b.message));

            prop_assert_eq!(actual, expected);
        }

        #[test]
        fn missing_span(
            (file, id) in mock_file_strategy().prop_flat_map(|file| {
                let mut ids = Vec::new();
                file.walk(|node, _| ids.push(node.counter));
                proptest::sample::select(ids).prop_map(move |id| (file.clone(), id))
            })
        ) {
            let (mut output, _) = file.render();
            output.spans.remove(&id);

            let result = std::panic::catch_unwind(
                std::panic::AssertUnwindSafe(|| extract(output))
            );
            let error = result.expect_err("expected a panic");
            let message = error.downcast_ref::<&str>()
                .copied()
                .or_else(|| error.downcast_ref::<String>().map(String::as_str))
                .unwrap();
            prop_assert!(
                message.contains(BUG_NO_SPAN_FOR_COUNTER),
                "unexpected panic message: {message:?}",
            );
        }

        #[test]
        fn extra_node_metadata(
            file in mock_file_strategy()
        ) {
            let (mut output, _) = file.render();
            let extra_counter = output
                .spans
                .keys()
                .chain(output.metadata.node.keys())
                .copied()
                .max()
                .map_or(0, |counter| counter + 1);
            output.metadata.node.insert(
                extra_counter,
                HashMap::from([("k".to_owned(), vec!["v".to_owned()])]),
            );

            let actual = extract(output).unwrap_err().to_vec();
            let expected = vec![orphaned_node_metadata_diagnostic(extra_counter)];

            prop_assert_eq!(actual, expected);
        }

        #[test]
        fn extra_transclusion_metadata(
            file in mock_file_strategy()
        ) {
            let (mut output, _) = file.render();
            let extra_counter = output
                .metadata.transclusion
                .keys()
                .copied()
                .chain(
                    transclude_counters(&output.html)
                        .into_iter()
                        .map(|counter| counter.parse::<u32>().unwrap()),
                )
                .max()
                .map_or(0, |counter| counter + 1);
            output.metadata.transclusion.insert(
                extra_counter,
                HashMap::from([("k".to_owned(), vec!["v".to_owned()])]),
            );

            let actual = extract(output).unwrap_err().to_vec();
            let expected = vec![orphaned_transclusion_metadata_diagnostic(extra_counter)];

            prop_assert_eq!(actual, expected);
        }

        #[test]
        fn extra_link_metadata(
            file in mock_file_strategy()
        ) {
            let (mut output, _) = file.render();
            let extra_counter = output
                .metadata.link
                .keys()
                .copied()
                .chain(
                    link_counters(&output.html)
                        .into_iter()
                        .map(|counter| counter.parse::<u32>().unwrap()),
                )
                .max()
                .map_or(0, |counter| counter + 1);
            output.metadata.link.insert(
                extra_counter,
                HashMap::from([("k".to_owned(), vec!["v".to_owned()])]),
            );

            let actual = extract(output).unwrap_err().to_vec();
            let expected = vec![orphaned_link_metadata_diagnostic(extra_counter)];

            prop_assert_eq!(actual, expected);
        }

        #[test]
        fn duplicate_node_identifier_is_preserved(
            (file, picked, k) in mock_file_strategy()
                .prop_flat_map(|file| {
                    let mut ids = Vec::new();
                    file.walk(|node, _| ids.push(node.identifier.clone()));
                    let k = (ids.len() / 2).max(1);
                    (Just(file), Just(ids), 1usize..=k)
                })
                .prop_filter("need at least 2 nodes for a duplicate pair", |(_, ids, _): &(MockFile, Vec<String>, usize)| {
                    ids.len() >= 2
                })
                .prop_flat_map(|(file, ids, k)| {
                    proptest::sample::subsequence(ids, 2 * k)
                        .prop_map(move |picked| (file.clone(), picked, k))
                })
        ) {
            let (mut output, _) = file.render();

            let document = Document::from(output.html.as_str());
            for (target_id, source_id) in picked[..k].iter().zip(picked[k..2 * k].iter()) {
                let selector = format!(
                    r#"wb-node[identifier="{target_id}"], wb-subnode[identifier="{target_id}"]"#
                );
                document.select(&selector).set_attr("identifier", source_id);
            }
            output.html = document.html().to_string();

            let actual = extract(output).unwrap();
            let identifiers: Vec<&str> = actual.iter().map(|node| node.0.as_str()).collect();
            for target_id in &picked[..k] {
                prop_assert_eq!(
                    identifiers.iter().filter(|&&id| id == target_id).count(),
                    0,
                    "target id should have been rewritten: {}",
                    target_id,
                );
            }
            for source_id in &picked[k..2 * k] {
                prop_assert!(
                    identifiers.iter().filter(|&&id| id == source_id).count() >= 2,
                    "duplicate id was not preserved: {}",
                    source_id,
                );
            }
        }

        #[test]
        fn missing_title(
            (file, ids) in mock_file_strategy()
                .prop_flat_map(|file| {
                    let mut ids = Vec::new();
                    file.walk(|node, is_subnode| ids.push((node.identifier.clone(), is_subnode)));
                    let n = ids.len();
                    proptest::sample::subsequence(ids, 1..=n)
                        .prop_map(move |stripped| (file.clone(), stripped))
                })
        ) {
            let (mut output, _) = file.render();

            let document = Document::from(output.html.as_str());
            for (id, _) in &ids {
                let selector =
                    format!(r#"wb-node[identifier="{id}"], wb-subnode[identifier="{id}"]"#);
                document.select(&selector).children().first().remove();
            }
            output.html = document.html().to_string();

            let mut expected: Vec<SourceDiagnostic> = ids
                .iter()
                .map(|(_, is_subnode)| missing_title_diagnostic(*is_subnode))
                .collect();
            let mut actual = extract(output).unwrap_err().to_vec();

            actual.sort_by(|a, b| a.message.cmp(&b.message));
            expected.sort_by(|a, b| a.message.cmp(&b.message));

            prop_assert_eq!(actual, expected);
        }

        #[test]
        fn missing_transclude_attribute(
            (file, ids) in mock_file_strategy()
                .prop_flat_map(|file| {
                    let mut ids = Vec::new();
                    file.walk(|node, is_subnode| {
                        if is_subnode {
                            ids.push(node.identifier.clone());
                        }
                    });
                    let n = ids.len();
                    proptest::sample::subsequence(ids, 0..=n)
                        .prop_map(move |stripped| (file.clone(), stripped))
                })
                .prop_filter("need at least one subnode", |(_, ids)| !ids.is_empty())
        ) {
            let (mut output, _) = file.render();

            let document = Document::from(output.html.as_str());
            for id in &ids {
                let selector = format!(r#"wb-subnode[identifier="{id}"]"#);
                document.select(&selector).remove_attr("transclude");
            }
            output.html = document.html().to_string();

            let mut expected: Vec<SourceDiagnostic> = ids
                .iter()
                .map(|_| SourceDiagnostic::error(Span::detached(), WB_SUBNODE_MISSING_TRANSCLUDE))
                .collect();
            let mut actual = extract(output).unwrap_err().to_vec();

            actual.sort_by(|a, b| a.message.cmp(&b.message));
            expected.sort_by(|a, b| a.message.cmp(&b.message));

            prop_assert_eq!(actual, expected);
        }

        #[test]
        fn invalid_transclude_attribute_value(
            (file, ids, values) in mock_file_strategy()
                .prop_flat_map(|file| {
                    let mut ids = Vec::new();
                    file.walk(|node, is_subnode| {
                        if is_subnode {
                            ids.push(node.identifier.clone());
                        }
                    });
                    let n = ids.len();
                    proptest::sample::subsequence(ids, 0..=n)
                        .prop_map(move |stripped| (file.clone(), stripped))
                })
                .prop_filter("need at least one subnode", |(_, ids)| !ids.is_empty())
                .prop_flat_map(|(file, ids)| {
                    let n = ids.len();
                    proptest::collection::vec(
                        "[a-z]{1,8}".prop_filter("not true or false", |s| {
                            s != "true" && s != "false"
                        }),
                        n,
                    )
                    .prop_map(move |values| (file.clone(), ids.clone(), values))
                })
        ) {
            let (mut output, _) = file.render();

            let document = Document::from(output.html.as_str());
            for (id, value) in ids.iter().zip(values.iter()) {
                let selector = format!(r#"wb-subnode[identifier="{id}"]"#);
                document.select(&selector).set_attr("transclude", value.as_str());
            }
            output.html = document.html().to_string();

            let mut expected: Vec<SourceDiagnostic> = values
                .iter()
                .map(|v| invalid_transclude_value_diagnostic(v))
                .collect();
            let mut actual = extract(output).unwrap_err().to_vec();

            actual.sort_by(|a, b| a.message.cmp(&b.message));
            expected.sort_by(|a, b| a.message.cmp(&b.message));

            prop_assert_eq!(actual, expected);
        }

        #[test]
        fn missing_transclude_identifier(
            (mut output, counters) in mock_file_strategy()
                .prop_flat_map(|file| {
                    let (output, _) = file.render();
                    let counters = transclude_counters(&output.html);
                    let n = counters.len();
                    proptest::sample::subsequence(counters, 0..=n)
                        .prop_map(move |selected| (output.clone(), selected))
                })
                .prop_filter("need at least one transclude", |(_, counters)| !counters.is_empty())
        ) {
            let document = Document::from(output.html.as_str());
            let mut expected: Vec<SourceDiagnostic> = Vec::new();
            for counter in &counters {
                let selector = format!(r#"wb-transclude[counter="{counter}"]"#);
                document.select(&selector).remove_attr("identifier");
                let counter_u32: u32 = counter.parse().unwrap();
                if output.metadata.transclusion.contains_key(&counter_u32) {
                    expected.push(orphaned_transclusion_metadata_diagnostic(counter_u32));
                }
                expected.push(SourceDiagnostic::error(Span::detached(), WB_TRANSCLUDE_MISSING_IDENTIFIER));
            }
            output.html = document.html().to_string();

            let mut actual = extract(output).unwrap_err().to_vec();

            actual.sort_by(|a, b| a.message.cmp(&b.message));
            expected.sort_by(|a, b| a.message.cmp(&b.message));

            prop_assert_eq!(actual, expected);
        }

        #[test]
        fn missing_transclude_counter(
            (file, counters) in mock_file_strategy()
                .prop_flat_map(|file| {
                    let (output, _) = file.render();
                    let counters = transclude_counters(&output.html);
                    let n = counters.len();
                    proptest::sample::subsequence(counters, 0..=n)
                        .prop_map(move |selected| (file.clone(), selected))
                })
                .prop_filter("need at least one transclude", |(_, counters)| !counters.is_empty())
        ) {
            let (mut output, _) = file.render();

            let document = Document::from(output.html.as_str());
            let mut expected: Vec<SourceDiagnostic> = Vec::new();
            for counter in &counters {
                let selector = format!(r#"wb-transclude[counter="{counter}"]"#);
                document.select(&selector).remove_attr("counter");
                let counter_u32: u32 = counter.parse().unwrap();
                if output.metadata.transclusion.contains_key(&counter_u32) {
                    expected.push(orphaned_transclusion_metadata_diagnostic(counter_u32));
                }
                expected.push(SourceDiagnostic::error(Span::detached(), WB_TRANSCLUDE_MISSING_COUNTER));
            }
            output.html = document.html().to_string();

            let mut actual = extract(output).unwrap_err().to_vec();

            actual.sort_by(|a, b| a.message.cmp(&b.message));
            expected.sort_by(|a, b| a.message.cmp(&b.message));

            prop_assert_eq!(actual, expected);
        }

        #[test]
        fn invalid_transclude_counter(
            (file, counters, values) in mock_file_strategy()
                .prop_flat_map(|file| {
                    let (output, _) = file.render();
                    let counters = transclude_counters(&output.html);
                    let n = counters.len();
                    proptest::sample::subsequence(counters, 0..=n)
                        .prop_map(move |selected| (file.clone(), selected))
                })
                .prop_filter("need at least one transclude", |(_, counters)| !counters.is_empty())
                .prop_flat_map(|(file, counters)| {
                    let n = counters.len();
                    proptest::collection::vec(
                        "[a-z]{1,8}",
                        n,
                    )
                    .prop_map(move |values| (file.clone(), counters.clone(), values))
                })
        ) {
            let (mut output, _) = file.render();

            let document = Document::from(output.html.as_str());
            let mut expected: Vec<SourceDiagnostic> = Vec::new();
            for (counter, value) in counters.iter().zip(values.iter()) {
                let selector = format!(r#"wb-transclude[counter="{counter}"]"#);
                document.select(&selector).set_attr("counter", value.as_str());
                let counter_u32: u32 = counter.parse().unwrap();
                if output.metadata.transclusion.contains_key(&counter_u32) {
                    expected.push(orphaned_transclusion_metadata_diagnostic(counter_u32));
                }
                expected.push(invalid_transclude_counter_diagnostic(value));
            }
            output.html = document.html().to_string();

            let mut actual = extract(output).unwrap_err().to_vec();

            actual.sort_by(|a, b| a.message.cmp(&b.message));
            expected.sort_by(|a, b| a.message.cmp(&b.message));

            prop_assert_eq!(actual, expected);
        }

        #[test]
        fn missing_link_counter(
            (file, counters) in mock_file_strategy()
                .prop_flat_map(|file| {
                    let (output, _) = file.render();
                    let counters = link_counters(&output.html);
                    let n = counters.len();
                    proptest::sample::subsequence(counters, 0..=n)
                        .prop_map(move |selected| (file.clone(), selected))
                })
                .prop_filter("need at least one link", |(_, counters)| !counters.is_empty())
        ) {
            let (mut output, _) = file.render();

            let document = Document::from(output.html.as_str());
            let mut expected: Vec<SourceDiagnostic> = Vec::new();
            for counter in &counters {
                let selector = format!(r#"a[data-counter="{counter}"]"#);
                document.select(&selector).remove_attr("data-counter");
                let counter_u32: u32 = counter.parse().unwrap();
                if output.metadata.link.contains_key(&counter_u32) {
                    expected.push(orphaned_link_metadata_diagnostic(counter_u32));
                }
                expected.push(SourceDiagnostic::error(Span::detached(), LINK_MISSING_COUNTER));
            }
            output.html = document.html().to_string();

            let mut actual = extract(output).unwrap_err().to_vec();

            actual.sort_by(|a, b| a.message.cmp(&b.message));
            expected.sort_by(|a, b| a.message.cmp(&b.message));

            prop_assert_eq!(actual, expected);
        }

        #[test]
        fn invalid_link_counter(
            (file, counters, values) in mock_file_strategy()
                .prop_flat_map(|file| {
                    let (output, _) = file.render();
                    let counters = link_counters(&output.html);
                    let n = counters.len();
                    proptest::sample::subsequence(counters, 0..=n)
                        .prop_map(move |selected| (file.clone(), selected))
                })
                .prop_filter("need at least one link", |(_, counters)| !counters.is_empty())
                .prop_flat_map(|(file, counters)| {
                    let n = counters.len();
                    proptest::collection::vec(
                        "[a-z]{1,8}",
                        n,
                    )
                    .prop_map(move |values| (file.clone(), counters.clone(), values))
                })
        ) {
            let (mut output, _) = file.render();

            let document = Document::from(output.html.as_str());
            let mut expected: Vec<SourceDiagnostic> = Vec::new();
            for (counter, value) in counters.iter().zip(values.iter()) {
                let selector = format!(r#"a[data-counter="{counter}"]"#);
                document.select(&selector).set_attr("data-counter", value.as_str());
                let counter_u32: u32 = counter.parse().unwrap();
                if output.metadata.link.contains_key(&counter_u32) {
                    expected.push(orphaned_link_metadata_diagnostic(counter_u32));
                }
                expected.push(invalid_link_counter_diagnostic(value));
            }
            output.html = document.html().to_string();

            let mut actual = extract(output).unwrap_err().to_vec();

            actual.sort_by(|a, b| a.message.cmp(&b.message));
            expected.sort_by(|a, b| a.message.cmp(&b.message));

            prop_assert_eq!(actual, expected);
        }
    }
}
