use std::collections::HashMap;

use super::super::Metadata;
use super::super::render::{
    BackmatterInput, BackmatterNode, BodyInput, LinkInput, NodeInput, Render, ResolvedLink,
    ResolvedTransclusion, TransclusionInput,
};

pub struct MockRenderer;

// The code here is extra verbose because it purposefully destructs each of the
// types from the render trait to ensure we don't miss anything.
impl Render for MockRenderer {
    type Body = RenderBody;
    type Backmatter = RenderBackmatter;
    type Node = RenderNode;

    fn render_body(&self, input: BodyInput<'_, RenderBody>) -> anyhow::Result<RenderBody> {
        let BodyInput {
            body_html,
            links,
            transclusions,
        } = input;
        Ok(RenderBody {
            body_html: body_html.to_owned(),
            links: links
                .into_iter()
                .map(|(k, v)| {
                    let LinkInput {
                        identifier,
                        metadata,
                        resolution,
                    } = v;
                    (
                        k,
                        RenderLinkInput {
                            identifier: identifier.to_owned(),
                            metadata: metadata.cloned(),
                            resolution: resolution.map(|r| {
                                let ResolvedLink {
                                    title,
                                    title_text,
                                    metadata,
                                } = r;
                                RenderResolvedLink {
                                    title: title.to_owned(),
                                    title_text: title_text.to_owned(),
                                    metadata: metadata.clone(),
                                }
                            }),
                        },
                    )
                })
                .collect(),
            transclusions: transclusions
                .into_iter()
                .map(|(k, v)| {
                    let TransclusionInput {
                        metadata,
                        resolution,
                    } = v;
                    (
                        k,
                        RenderTransclusionInput {
                            metadata: metadata.cloned(),
                            resolution: resolution.map(|r| {
                                let ResolvedTransclusion {
                                    identifier,
                                    title,
                                    title_text,
                                    metadata,
                                    body,
                                } = r;
                                RenderResolvedTransclusion {
                                    identifier: identifier.to_owned(),
                                    title: title.to_owned(),
                                    title_text: title_text.to_owned(),
                                    metadata: metadata.clone(),
                                    body: Box::new(body.clone()),
                                }
                            }),
                        },
                    )
                })
                .collect(),
        })
    }

    fn render_backmatter(&self, input: BackmatterInput<'_>) -> anyhow::Result<RenderBackmatter> {
        fn to_render_backmatter_node(n: BackmatterNode<'_>) -> RenderBackmatterNode {
            let BackmatterNode {
                title,
                title_text,
                metadata,
            } = n;
            RenderBackmatterNode {
                title: title.to_owned(),
                title_text: title_text.to_owned(),
                metadata: metadata.clone(),
            }
        }

        let BackmatterInput {
            node,
            contexts,
            backlinks,
            outlinks,
        } = input;
        Ok(RenderBackmatter {
            node: (node.0, to_render_backmatter_node(node.1)),
            contexts: contexts
                .into_iter()
                .map(|(id, opt)| (id, opt.map(to_render_backmatter_node)))
                .collect(),
            backlinks: backlinks
                .into_iter()
                .map(|(id, opt)| (id, opt.map(to_render_backmatter_node)))
                .collect(),
            outlinks: outlinks
                .into_iter()
                .map(|(id, opt)| (id, opt.map(to_render_backmatter_node)))
                .collect(),
        })
    }

    fn render_node(
        &self,
        input: NodeInput<'_, RenderBody, RenderBackmatter>,
    ) -> anyhow::Result<RenderNode> {
        let NodeInput {
            identifier,
            title,
            title_text,
            metadata,
            body,
            backmatter,
        } = input;
        Ok(RenderNode {
            identifier,
            title: title.to_owned(),
            title_text: title_text.to_owned(),
            metadata: metadata.clone(),
            body: body.clone(),
            backmatter: backmatter.clone(),
        })
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RenderBody {
    pub body_html: String,
    pub links: HashMap<u32, RenderLinkInput>,
    pub transclusions: HashMap<u32, RenderTransclusionInput>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RenderLinkInput {
    pub identifier: String,
    pub metadata: Option<Metadata>,
    pub resolution: Option<RenderResolvedLink>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RenderResolvedLink {
    pub title: String,
    pub title_text: String,
    pub metadata: Metadata,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RenderTransclusionInput {
    pub metadata: Option<Metadata>,
    pub resolution: Option<RenderResolvedTransclusion>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RenderResolvedTransclusion {
    pub identifier: String,
    pub title: String,
    pub title_text: String,
    pub metadata: Metadata,
    pub body: Box<RenderBody>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RenderBackmatterNode {
    pub title: String,
    pub title_text: String,
    pub metadata: Metadata,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RenderBackmatter {
    pub node: (String, RenderBackmatterNode),
    pub contexts: Vec<(String, Option<RenderBackmatterNode>)>,
    pub backlinks: Vec<(String, Option<RenderBackmatterNode>)>,
    pub outlinks: Vec<(String, Option<RenderBackmatterNode>)>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RenderNode {
    pub identifier: String,
    pub title: String,
    pub title_text: String,
    pub metadata: Metadata,
    pub body: RenderBody,
    pub backmatter: RenderBackmatter,
}
