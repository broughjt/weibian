use std::collections::HashMap;
use std::collections::hash_map::Entry;

use dom_query::{Document, Selection};
use ecow::{EcoVec, eco_format};
use typst::World;
use typst::diag::{Severity, SourceDiagnostic, Warned};
use typst::foundations::{Dict, NativeElement, Packed, Repr, Value};
use typst::introspection::{Introspector, MetadataElem};
use typst::syntax::Span;
use typst_html::{HtmlAttr, HtmlDocument, HtmlNode, HtmlTag};

use super::NodeEntry;

const HTML_MESSAGE: &str = "html export is under active development and incomplete";

/// Compiles a source file and extracts its nodes.
pub trait Compile {
    fn compile(self) -> Warned<Result<HashMap<String, NodeOutput>, EcoVec<SourceDiagnostic>>>;
}

pub struct NodeOutput {
    pub(super) entry: NodeEntry,
    pub transclusions: Vec<String>,
    pub links: Vec<String>,
}

struct FileOutput {
    html: String,
    spans: HashMap<String, Span>,
    node_metadata: HashMap<String, HashMap<String, Vec<String>>>,
    transclusion_metadata: HashMap<u32, HashMap<String, Vec<String>>>,
    link_metadata: HashMap<u32, HashMap<String, Vec<String>>>,
}

/// Wraps a Typst [`World`] so it can be passed to [`Compiler::update`].
pub struct TypstCompile<W>(pub W);

impl<W: World> Compile for TypstCompile<W> {
    fn compile(self) -> Warned<Result<HashMap<String, NodeOutput>, EcoVec<SourceDiagnostic>>> {
        let Warned {
            output: result,
            mut warnings,
        } = typst::compile::<typst_html::HtmlDocument>(&self.0);

        // Discard warnings about html being an unstable feature, html is kind
        // of the whole game here
        warnings.retain(|diagnostic: &mut SourceDiagnostic| {
            !(diagnostic.severity == Severity::Warning && diagnostic.message == HTML_MESSAGE)
        });

        let output = result.and_then(|html_document| {
            typst_html::html(&html_document).and_then(|html| {
                let (spans, span_errors) = collect_node_spans(&html_document);
                let (metadata, transclusion_metadata, link_metadata, meta_errors) =
                    collect_metadata(html_document.introspector().as_ref(), &spans);
                let mut errors = span_errors;
                errors.extend(meta_errors);

                if errors.is_empty() {
                    let compile_output = FileOutput {
                        html,
                        spans,
                        node_metadata: metadata,
                        transclusion_metadata,
                        link_metadata,
                    };

                    extract(compile_output)
                } else {
                    Err(errors)
                }
            })
        });

        Warned { output, warnings }
    }
}

/// Parses the HTML in `output` into a map of node IDs to node entries.
///
/// Returns `Err` with all collected diagnostics if any validation errors occur,
/// or `Ok` with the node map on success.
fn extract(output: FileOutput) -> Result<HashMap<String, NodeOutput>, EcoVec<SourceDiagnostic>> {
    let FileOutput {
        html,
        spans,
        node_metadata: mut metadata,
        mut transclusion_metadata,
        mut link_metadata,
    } = output;
    let mut errors = EcoVec::new();
    let document = Document::from(html);
    let mut nodes = HashMap::with_capacity(spans.len());
    let mut synthetic_counter: u32 = transclusion_metadata.keys().copied().max().map_or(0, |m| {
        m.checked_add(1).expect("transclusion counter overflow")
    });

    // Process subnodes deepest-first: reversed pre-order ensures a
    // nested subnode is always processed before its parent subnode.
    for subnode in document.select("wb-subnode").iter().rev() {
        let Some((identifier, output)) = extract_node_content(
            &subnode,
            true,
            &spans,
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
                    output.entry.span,
                    eco_format!("wb-subnode has invalid transclude value: {other:?}"),
                ));
                continue;
            }
            None => {
                errors.push(SourceDiagnostic::error(
                    output.entry.span,
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

            transclusion_metadata.insert(counter, output.entry.node_metadata.clone());
            subnode.replace_with_html(format!(
                r#"<wb-transclude identifier="{identifier}" counter="{counter}"></wb-transclude>"#
            ));
        } else {
            subnode.remove();
        }

        let displaced = nodes.insert(identifier, output);
        assert!(
            displaced.is_none(),
            "bug: duplicate node identifier slipped past collect_node_spans"
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
            if let Some((identifier, output)) = extract_node_content(
                &wb_node,
                false,
                &spans,
                &mut metadata,
                &mut transclusion_metadata,
                &mut link_metadata,
                &mut errors,
            ) {
                let displaced = nodes.insert(identifier, output);
                assert!(
                    displaced.is_none(),
                    "bug: duplicate node identifier slipped past collect_node_spans"
                );
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
/// [`SourceNode`], collecting its transclusions and links and consuming its
/// metadata from the provided map.
///
/// Returns `None` (pushing an error) if the identifier attribute is missing or
/// if the element's first child is not a `wb-title` element.
#[allow(clippy::too_many_arguments)]
fn extract_node_content(
    element: &Selection,
    is_subnode: bool,
    spans: &HashMap<String, Span>,
    metadata: &mut HashMap<String, HashMap<String, Vec<String>>>,
    transclusion_metadata: &mut HashMap<u32, HashMap<String, Vec<String>>>,
    link_metadata: &mut HashMap<u32, HashMap<String, Vec<String>>>,
    errors: &mut EcoVec<SourceDiagnostic>,
) -> Option<(String, NodeOutput)> {
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

    let body_html = element.inner_html().to_string();

    let mut transclusions: Vec<String> = Vec::new();
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
        transclusions.push(id);
    }

    let mut links: Vec<String> = Vec::new();
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
        links.push(id);
    }

    let node_metadata = metadata.remove(&identifier).unwrap_or_default();

    Some((
        identifier,
        NodeOutput {
            entry: NodeEntry {
                body_html,
                title,
                title_text,
                span,
                node_metadata,
                transclusion_metadata: node_transclusion_metadata,
                link_metadata: node_link_metadata,
            },
            transclusions,
            links,
        },
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
pub(super) fn collect_metadata<I: Introspector>(
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
pub(super) fn collect_node_spans(
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
