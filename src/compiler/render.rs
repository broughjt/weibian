use std::collections::HashMap;

use dom_query::Document;

use crate::config::RenderConfig;

use super::{NodeEntry, NodeId, NodeInterner};

pub(super) fn render_body(
    id: NodeId,
    nodes: &HashMap<NodeId, NodeEntry>,
    interner: &NodeInterner,
    link_template: &minijinja::Template<'_, '_>,
    transclusion_template: &minijinja::Template<'_, '_>,
    config: &RenderConfig,
    site_context: &minijinja::Value,
) -> anyhow::Result<String> {
    let entry = &nodes[&id];
    let document = Document::from(entry.raw_html.as_str());

    // Render internal links. Done before transclusion substitution so that
    // links inside already-rendered transclusion bodies are not double-processed.
    for (element, identifier) in document.select("a").iter().filter_map(|element| {
        element
            .attr("href")
            .and_then(|href| href.strip_prefix("wb:").map(ToOwned::to_owned))
            .map(|identifier| (element, identifier))
    }) {
        let counter: u32 = element
            .attr("data-counter")
            .expect("bug: link is missing a data-counter")
            .parse()
            .expect("bug: link has invalid data-counter");
        let href = minijinja::Value::from_safe_string(config.href(&identifier));
        let content = element.inner_html().to_string();
        let link_metadata = entry
            .link_metadata
            .get(&counter)
            .cloned()
            .unwrap_or_default();
        let context = if let Some(target_id) = interner.get(&identifier)
            && let Some(target) = nodes.get(&target_id)
        {
            minijinja::context! {
                link => minijinja::context! {
                    identifier => identifier,
                    href => &href,
                    content => content,
                    resolved => true,
                    title => target.title.as_str(),
                    title_text => target.title_text.as_str(),
                    metadata => target.metadata,
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
        let replacement = link_template
            .render(context)
            .map_err(|e| anyhow::anyhow!("failed to render link template for {identifier}: {e}"))?;
        element.replace_with_html(replacement);
    }

    for element in document.select("wb-transclude").iter() {
        let identifier = element
            .attr("identifier")
            .expect("bug: wb-transclude is missing an identifier");
        let counter: u32 = element
            .attr("counter")
            .expect("bug: wb-transclude is missing a counter")
            .parse()
            .expect("bug: wb-transclude has invalid counter");
        let transclusion_metadata = entry
            .transclusion_metadata
            .get(&counter)
            .cloned()
            .unwrap_or_default();
        let transclude_id = interner
            .get(identifier.as_ref())
            .expect("bug: wb-transclude identifier was not interned");
        let context = if let Some(target) = nodes.get(&transclude_id) {
            let body = target
                .rendered_body
                .as_deref()
                .expect("bug: wb-transclude target has no rendered_body");
            minijinja::context! {
                transclusion => minijinja::context! {
                    identifier => identifier.as_ref(),
                    href => minijinja::Value::from_safe_string(config.href(identifier.as_ref())),
                    resolved => true,
                    title => target.title.as_str(),
                    title_text => target.title_text.as_str(),
                    body => body,
                    metadata => target.metadata,
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
            anyhow::anyhow!("failed to render transclusion template for {identifier}: {e}")
        })?;
        element.replace_with_html(replacement);
    }

    Ok(document.select("body").first().inner_html().to_string())
}

pub(super) fn render_backmatter(
    id: NodeId,
    nodes: &HashMap<NodeId, NodeEntry>,
    interner: &NodeInterner,
    backmatter_template: &minijinja::Template<'_, '_>,
    config: &RenderConfig,
    site_context: &minijinja::Value,
) -> anyhow::Result<String> {
    let cache = nodes[&id]
        .backmatter_cache
        .as_ref()
        .expect("bug: backmatter_render node has no backmatter_cache");

    let node_info = |node_id: NodeId| {
        let name = interner.name(node_id);
        let entry = nodes.get(&node_id);
        minijinja::context! {
            id => name,
            href => minijinja::Value::from_safe_string(config.href(name)),
            title => entry.map(|e| e.title.as_str()).unwrap_or(""),
            title_text => entry.map(|e| e.title_text.as_str()).unwrap_or(""),
            metadata => entry.map(|e| &e.metadata),
        }
    };

    let mut contexts_ids: Vec<NodeId> = cache.contexts.iter().copied().collect();
    contexts_ids.sort_by_key(|&nid| interner.name(nid));
    let contexts: Vec<_> = contexts_ids.into_iter().map(&node_info).collect();

    let mut backlinks_ids: Vec<NodeId> = cache.backlinks.iter().copied().collect();
    backlinks_ids.sort_by_key(|&nid| interner.name(nid));
    let backlinks: Vec<_> = backlinks_ids.into_iter().map(&node_info).collect();

    let mut outlinks_ids: Vec<NodeId> = cache.outlinks.iter().copied().collect();
    outlinks_ids.sort_by_key(|&nid| interner.name(nid));
    let outlinks: Vec<_> = outlinks_ids.into_iter().map(&node_info).collect();

    let name = interner.name(id);
    backmatter_template
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
        .map_err(|e| anyhow::anyhow!("failed to render backmatter template for {name}: {e}"))
}

pub(super) fn render_node(
    name: &str,
    entry: &NodeEntry,
    body: &str,
    backmatter: &str,
    node_template: &minijinja::Template<'_, '_>,
    config: &RenderConfig,
    site_context: &minijinja::Value,
) -> anyhow::Result<String> {
    node_template
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
        .map_err(|e| anyhow::anyhow!("failed to render template for node {name}: {e}"))
}
