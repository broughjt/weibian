use std::collections::HashMap;

use dom_query::{Document, Selection};

use crate::config::{
    BACKMATTER_TEMPLATE, LINK_TEMPLATE, NODE_TEMPLATE, RenderConfig, TRANSCLUSION_TEMPLATE,
};

use super::{Backmatter, NodeEntry, NodeId, NodeInterner};

pub(super) struct Renderer<'a> {
    nodes: &'a HashMap<NodeId, NodeEntry>,
    interner: &'a NodeInterner,
    config: &'a RenderConfig,
    site_context: minijinja::Value,
    link_template: minijinja::Template<'a, 'a>,
    transclusion_template: minijinja::Template<'a, 'a>,
    backmatter_template: minijinja::Template<'a, 'a>,
    node_template: minijinja::Template<'a, 'a>,
}

impl<'a> Renderer<'a> {
    pub(super) fn new(
        nodes: &'a HashMap<NodeId, NodeEntry>,
        interner: &'a NodeInterner,
        config: &'a RenderConfig,
    ) -> Self {
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

        Self {
            nodes,
            interner,
            config,
            site_context,
            link_template,
            transclusion_template,
            backmatter_template,
            node_template,
        }
    }

    pub(super) fn render_body(
        &self,
        id: NodeId,
        rendered_bodies: &HashMap<NodeId, String>,
    ) -> anyhow::Result<String> {
        let entry = &self.nodes[&id];
        let document = Document::from(entry.body_html.as_str());

        // Render internal links. Done before transclusion substitution so that
        // links inside already-rendered transclusion bodies are not double-processed.
        for (element, identifier) in document.select("a").iter().filter_map(|element| {
            element
                .attr("href")
                .and_then(|href| href.strip_prefix("wb:").map(ToOwned::to_owned))
                .map(|identifier| (element, identifier))
        }) {
            element.replace_with_html(self.render_link(&element, &identifier, entry)?);
        }

        for element in document.select("wb-transclude").iter() {
            element.replace_with_html(self.render_transclusion(
                &element,
                entry,
                rendered_bodies,
            )?);
        }

        Ok(document.select("body").first().inner_html().to_string())
    }

    pub(super) fn render_backmatter(
        &self,
        id: NodeId,
        backmatter: &Backmatter,
    ) -> anyhow::Result<String> {
        let node_context = |node_id: NodeId| {
            let name = self.interner.name(node_id);
            let entry = self.nodes.get(&node_id);

            minijinja::context! {
                id => name,
                href => minijinja::Value::from_safe_string(self.config.href(name)),
                title => entry.map(|e| e.title.as_str()).unwrap_or_default(),
                title_text => entry.map(|e| e.title_text.as_str()).unwrap_or_default(),
                metadata => entry.map(|e| &e.node_metadata),
            }
        };

        let mut contexts_ids: Vec<NodeId> = backmatter.contexts.iter().copied().collect();
        contexts_ids.sort_by_key(|&nid| self.interner.name(nid));
        let contexts: Vec<_> = contexts_ids.into_iter().map(&node_context).collect();

        let mut backlinks_ids: Vec<NodeId> = backmatter.backlinks.iter().copied().collect();
        backlinks_ids.sort_by_key(|&nid| self.interner.name(nid));
        let backlinks: Vec<_> = backlinks_ids.into_iter().map(&node_context).collect();

        let mut outlinks_ids: Vec<NodeId> = backmatter.outlinks.iter().copied().collect();
        outlinks_ids.sort_by_key(|&nid| self.interner.name(nid));
        let outlinks: Vec<_> = outlinks_ids.into_iter().map(&node_context).collect();

        let name = self.interner.name(id);
        self.backmatter_template
            .render(minijinja::context! {
                backmatter => minijinja::context! {
                    contexts => contexts,
                    backlinks => backlinks,
                    outlinks => outlinks,
                },
                node => node_context(id),
                site => &self.site_context,
            })
            .map_err(|e| anyhow::anyhow!("failed to render backmatter template for {name}: {e}"))
    }

    pub(super) fn render_node(
        &self,
        name: &str,
        entry: &NodeEntry,
        body: &str,
        backmatter: &str,
    ) -> anyhow::Result<String> {
        self.node_template
            .render(minijinja::context! {
                node => minijinja::context! {
                    id => name,
                    href => minijinja::Value::from_safe_string(self.config.href(name)),
                    title => entry.title.as_str(),
                    title_text => entry.title_text.as_str(),
                    body => body,
                    backmatter => backmatter,
                    metadata => entry.node_metadata,
                },
                site => &self.site_context,
            })
            .map_err(|e| anyhow::anyhow!("failed to render template for node {name}: {e}"))
    }

    fn render_link(
        &self,
        element: &Selection,
        identifier: &str,
        entry: &NodeEntry,
    ) -> anyhow::Result<String> {
        let counter: u32 = element
            .attr("data-counter")
            .expect("bug: link is missing a data-counter")
            .parse()
            .expect("bug: link has invalid data-counter");
        let href = minijinja::Value::from_safe_string(self.config.href(identifier));
        let content = element.inner_html().to_string();
        let link_metadata = entry
            .link_metadata
            .get(&counter)
            .cloned()
            .unwrap_or_default();
        let link_id = self
            .interner
            .get(identifier.as_ref())
            .expect("bug: wb-transclude identifier was not interned");
        let context = if let Some(target) = self.nodes.get(&link_id) {
            minijinja::context! {
                link => minijinja::context! {
                    identifier => identifier,
                    href => &href,
                    content => content,
                    resolved => true,
                    title => target.title.as_str(),
                    title_text => target.title_text.as_str(),
                    metadata => target.node_metadata,
                    link_metadata => link_metadata,
                },
                site => &self.site_context,
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
                site => &self.site_context,
            }
        };

        self.link_template
            .render(context)
            .map_err(|e| anyhow::anyhow!("failed to render link template for {identifier}: {e}"))
    }

    fn render_transclusion(
        &self,
        element: &Selection,
        entry: &NodeEntry,
        rendered_bodies: &HashMap<NodeId, String>,
    ) -> anyhow::Result<String> {
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
        let transclude_id = self
            .interner
            .get(identifier.as_ref())
            .expect("bug: wb-transclude identifier was not interned");
        let context = if let Some(target) = self.nodes.get(&transclude_id) {
            let body = rendered_bodies
                .get(&transclude_id)
                .map(String::as_str)
                .expect("bug: wb-transclude target has no rendered_body");
            minijinja::context! {
                transclusion => minijinja::context! {
                    identifier => identifier.as_ref(),
                    href => minijinja::Value::from_safe_string(self.config.href(identifier.as_ref())),
                    resolved => true,
                    title => target.title.as_str(),
                    title_text => target.title_text.as_str(),
                    body => body,
                    metadata => target.node_metadata,
                    transclusion_metadata => transclusion_metadata,
                },
                site => &self.site_context,
            }
        } else {
            minijinja::context! {
                transclusion => minijinja::context! {
                    identifier => identifier.as_ref(),
                    resolved => false,
                    transclusion_metadata => transclusion_metadata,
                },
                site => &self.site_context,
            }
        };

        self.transclusion_template.render(context).map_err(|e| {
            anyhow::anyhow!("failed to render transclusion template for {identifier}: {e}")
        })
    }
}
