use std::collections::HashMap;

use dom_query::Document;

use crate::config::{
    BACKMATTER_TEMPLATE, LINK_TEMPLATE, NODE_TEMPLATE, RenderConfig, TRANSCLUSION_TEMPLATE,
};

use super::Metadata;

pub trait Render {
    type Body;
    type Backmatter;
    type Node;

    fn render_body(&self, input: BodyInput<'_, Self::Body>) -> anyhow::Result<Self::Body>;
    fn render_backmatter(&self, input: BackmatterInput<'_>) -> anyhow::Result<Self::Backmatter>;
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

pub struct JinjaRenderer<'a> {
    config: &'a RenderConfig,
    site_context: minijinja::Value,
    link_template: minijinja::Template<'a, 'a>,
    transclusion_template: minijinja::Template<'a, 'a>,
    backmatter_template: minijinja::Template<'a, 'a>,
    node_template: minijinja::Template<'a, 'a>,
}

impl<'a> JinjaRenderer<'a> {
    pub fn new(config: &'a RenderConfig) -> Self {
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
            config,
            site_context,
            link_template,
            transclusion_template,
            backmatter_template,
            node_template,
        }
    }
}

impl Render for JinjaRenderer<'_> {
    type Body = String;
    type Backmatter = String;
    type Node = String;

    fn render_body(&self, input: BodyInput<'_, String>) -> anyhow::Result<String> {
        let document = Document::from(input.body_html);

        // Render internal links before transclusions so that links inside
        // already-substituted transclusion bodies are not double-processed.
        for element in document.select("a").iter() {
            let Some(href) = element.attr("href") else {
                continue;
            };
            if href.strip_prefix("wb:").is_none() {
                continue;
            }
            let counter: u32 = element
                .attr("data-counter")
                .expect("bug: link missing data-counter")
                .parse()
                .expect("bug: link has invalid data-counter");

            let link_input = input
                .links
                .get(&counter)
                .expect("bug: link counter has no LinkInput");

            let content = element.inner_html().to_string();
            let href = minijinja::Value::from_safe_string(self.config.href(link_input.identifier));
            let link_metadata = link_input.metadata;

            let context = if let Some(resolved) = &link_input.resolution {
                minijinja::context! {
                    link => minijinja::context! {
                        identifier => link_input.identifier,
                        href => &href,
                        content => content,
                        resolved => true,
                        title => resolved.title,
                        title_text => resolved.title_text,
                        metadata => resolved.metadata,
                        link_metadata => link_metadata,
                    },
                    site => &self.site_context,
                }
            } else {
                minijinja::context! {
                    link => minijinja::context! {
                        identifier => link_input.identifier,
                        href => href,
                        content => content,
                        resolved => false,
                        link_metadata => link_metadata,
                    },
                    site => &self.site_context,
                }
            };

            let rendered = self.link_template.render(context).map_err(|e| {
                anyhow::anyhow!(
                    "failed to render link template for {}: {e}",
                    link_input.identifier
                )
            })?;
            element.replace_with_html(rendered);
        }

        for element in document.select("wb-transclude").iter() {
            let identifier_attr = element
                .attr("identifier")
                .expect("bug: wb-transclude missing identifier");
            let counter: u32 = element
                .attr("counter")
                .expect("bug: wb-transclude missing counter")
                .parse()
                .expect("bug: wb-transclude has invalid counter");

            let transclusion_input = input
                .transclusions
                .get(&counter)
                .expect("bug: transclusion counter has no TransclusionInput");

            let transclusion_metadata = transclusion_input.metadata;

            let context = if let Some(resolved) = &transclusion_input.resolution {
                let href =
                    minijinja::Value::from_safe_string(self.config.href(resolved.identifier));
                minijinja::context! {
                    transclusion => minijinja::context! {
                        identifier => resolved.identifier,
                        href => &href,
                        resolved => true,
                        title => resolved.title,
                        title_text => resolved.title_text,
                        body => resolved.body.as_str(),
                        metadata => resolved.metadata,
                        transclusion_metadata => transclusion_metadata,
                    },
                    site => &self.site_context,
                }
            } else {
                minijinja::context! {
                    transclusion => minijinja::context! {
                        identifier => identifier_attr.as_ref(),
                        resolved => false,
                        transclusion_metadata => transclusion_metadata,
                    },
                    site => &self.site_context,
                }
            };

            let rendered = self.transclusion_template.render(context).map_err(|e| {
                anyhow::anyhow!(
                    "failed to render transclusion template for {}: {e}",
                    identifier_attr.as_ref()
                )
            })?;
            element.replace_with_html(rendered);
        }

        Ok(document.select("body").first().inner_html().to_string())
    }

    fn render_backmatter(&self, input: BackmatterInput<'_>) -> anyhow::Result<String> {
        let node_context = |entry: &(String, Option<BackmatterNode<'_>>)| {
            let name = &entry.0;
            let option_node = entry.1;
            minijinja::context! {
                id => name.as_str(),
                href => minijinja::Value::from_safe_string(self.config.href(name)),
                title => option_node.map(|n| n.title).unwrap_or_default(),
                title_text => option_node.map(|n| n.title_text).unwrap_or_default(),
                metadata => option_node.map(|n| n.metadata),
            }
        };

        let contexts: Vec<_> = input.contexts.iter().map(node_context).collect();
        let backlinks: Vec<_> = input.backlinks.iter().map(node_context).collect();
        let outlinks: Vec<_> = input.outlinks.iter().map(node_context).collect();

        let self_context = minijinja::context! {
            id => input.node.0.as_str(),
            href => minijinja::Value::from_safe_string(self.config.href(&input.node.0)),
            title => input.node.1.title,
            title_text => input.node.1.title_text,
            metadata => input.node.1.metadata,
        };

        self.backmatter_template
            .render(minijinja::context! {
                backmatter => minijinja::context! {
                    contexts => contexts,
                    backlinks => backlinks,
                    outlinks => outlinks,
                },
                node => self_context,
                site => &self.site_context,
            })
            .map_err(|e| {
                anyhow::anyhow!(
                    "failed to render backmatter template for {}: {e}",
                    input.node.0
                )
            })
    }

    fn render_node(&self, input: NodeInput<'_, String, String>) -> anyhow::Result<String> {
        self.node_template
            .render(minijinja::context! {
                node => minijinja::context! {
                    id => input.identifier.as_str(),
                    href => minijinja::Value::from_safe_string(self.config.href(&input.identifier)),
                    title => input.title,
                    title_text => input.title_text,
                    body => input.body.as_str(),
                    backmatter => input.backmatter.as_str(),
                    metadata => input.metadata,
                },
                site => &self.site_context,
            })
            .map_err(|e| {
                anyhow::anyhow!(
                    "failed to render template for node {}: {e}",
                    input.identifier
                )
            })
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
