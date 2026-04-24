use std::collections::HashMap;

use dom_query::{Document, Selection};

use crate::config::{
    BACKMATTER_TEMPLATE, LINK_TEMPLATE, NODE_TEMPLATE, RenderConfig, TRANSCLUSION_TEMPLATE,
};

use super::{Backmatter, Metadata, NodeEntry, NodeId, NodeInterner};

pub struct JinjaRenderer<'a> {
    nodes: &'a HashMap<NodeId, NodeEntry>,
    interner: &'a NodeInterner,
    config: &'a RenderConfig,
    site_context: minijinja::Value,
    link_template: minijinja::Template<'a, 'a>,
    transclusion_template: minijinja::Template<'a, 'a>,
    backmatter_template: minijinja::Template<'a, 'a>,
    node_template: minijinja::Template<'a, 'a>,
}

impl<'a> JinjaRenderer<'a> {
    pub fn new(
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

    pub fn render_body(
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

    pub fn render_backmatter(&self, id: NodeId, backmatter: &Backmatter) -> anyhow::Result<String> {
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

    pub fn render_node(
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

pub trait Render {
    type Body;
    type Backmatter;
    type Node;

    fn render_body(&self, input: BodyInput<'_, Self::Body>) -> anyhow::Result<Self::Body>;
    fn render_backmatter(
        &self,
        input: BackmatterInput<'_>,
    ) -> anyhow::Result<Self::Backmatter>;
    fn render_node(
        &self,
        input: NodeInput<'_, Self::Body, Self::Backmatter>,
    ) -> anyhow::Result<Self::Node>;
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BodyInput<'a, Body> {
    pub body_html: &'a str,
    pub links: HashMap<u32, LinkInput<'a>>,
    pub transclusions: HashMap<u32, TransclusionInput<'a, Body>>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LinkInput<'a> {
    pub identifier: &'a str,
    pub metadata: Option<&'a Metadata>,
    pub resolution: Option<ResolvedLink<'a>>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ResolvedLink<'a> {
    pub title: &'a str,
    pub title_text: &'a str,
    pub metadata: &'a Metadata,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TransclusionInput<'a, Body> {
    pub metadata: Option<&'a Metadata>,
    pub resolution: Option<ResolvedTransclusion<'a, Body>>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ResolvedTransclusion<'a, Body> {
    pub identifier: &'a str,
    pub title: &'a str,
    pub title_text: &'a str,
    pub metadata: &'a Metadata,
    pub body: &'a Body,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BackmatterInput<'a> {
    pub node: (String, BackmatterNode<'a>),
    pub contexts: Vec<(String, Option<BackmatterNode<'a>>)>,
    pub backlinks: Vec<(String, Option<BackmatterNode<'a>>)>,
    pub outlinks: Vec<(String, Option<BackmatterNode<'a>>)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BackmatterNode<'a> {
    pub title: &'a str,
    pub title_text: &'a str,
    pub metadata: &'a Metadata,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NodeInput<'a, Body, Backmatter> {
    pub identifier: String,
    pub title: &'a str,
    pub title_text: &'a str,
    pub metadata: &'a Metadata,
    pub body: &'a Body,
    pub backmatter: &'a Backmatter,
}

impl Render for JinjaRenderer<'_> {
    type Body = String;
    type Backmatter = String;
    type Node = String;

    fn render_body(&self, input: BodyInput<'_, String>) -> anyhow::Result<String> {
        let _ = input;
        todo!()
    }

    fn render_backmatter(&self, input: BackmatterInput<'_>) -> anyhow::Result<String> {
        let _ = input;
        todo!()
    }

    fn render_node(
        &self,
        input: NodeInput<'_, String, String>,
    ) -> anyhow::Result<String> {
        let _ = input;
        todo!()
    }
}

// pub struct IdentityRenderer;

// /// Box breaks the `BodyInput<IdentityBody>` type recursion.
// #[derive(Clone, PartialEq, Eq, Debug)]
// pub struct IdentityBody(pub Box<BodyInput<IdentityBody>>);

// pub type IdentityBackmatter = BackmatterInput;
// pub type IdentityNode = NodeInput<IdentityBody, IdentityBackmatter>;

// impl Render for IdentityRenderer {
//     type Body = IdentityBody;
//     type Backmatter = IdentityBackmatter;
//     type Node = IdentityNode;

//     fn render_body(&self, input: BodyInput<IdentityBody>) -> anyhow::Result<IdentityBody> {
//         Ok(IdentityBody(Box::new(input)))
//     }

//     fn render_backmatter(&self, input: BackmatterInput) -> anyhow::Result<IdentityBackmatter> {
//         Ok(input)
//     }

//     fn render_node(
//         &self,
//         input: NodeInput<IdentityBody, IdentityBackmatter>,
//     ) -> anyhow::Result<IdentityNode> {
//         Ok(input)
//     }
// }
