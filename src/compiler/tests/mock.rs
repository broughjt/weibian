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
    type Body = MockBody;
    type Backmatter = MockBackmatter;
    type Node = MockNode;

    fn render_body(&self, input: BodyInput<'_, MockBody>) -> anyhow::Result<MockBody> {
        let BodyInput {
            body_html,
            links,
            transclusions,
        } = input;
        Ok(MockBody {
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
                        MockLinkInput {
                            identifier: identifier.to_owned(),
                            metadata: metadata.cloned(),
                            resolution: resolution.map(|r| {
                                let ResolvedLink {
                                    title,
                                    title_text,
                                    metadata,
                                } = r;
                                MockResolvedLink {
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
                        MockTransclusionInput {
                            metadata: metadata.cloned(),
                            resolution: resolution.map(|r| {
                                let ResolvedTransclusion {
                                    identifier,
                                    title,
                                    title_text,
                                    metadata,
                                    body,
                                } = r;
                                MockResolvedTransclusion {
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

    fn render_backmatter(&self, input: BackmatterInput<'_>) -> anyhow::Result<MockBackmatter> {
        fn to_mock_node(n: BackmatterNode<'_>) -> MockBackmatterNode {
            let BackmatterNode {
                title,
                title_text,
                metadata,
            } = n;
            MockBackmatterNode {
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
        Ok(MockBackmatter {
            node: (node.0, to_mock_node(node.1)),
            contexts: contexts
                .into_iter()
                .map(|(id, opt)| (id, opt.map(to_mock_node)))
                .collect(),
            backlinks: backlinks
                .into_iter()
                .map(|(id, opt)| (id, opt.map(to_mock_node)))
                .collect(),
            outlinks: outlinks
                .into_iter()
                .map(|(id, opt)| (id, opt.map(to_mock_node)))
                .collect(),
        })
    }

    fn render_node(
        &self,
        input: NodeInput<'_, MockBody, MockBackmatter>,
    ) -> anyhow::Result<MockNode> {
        let NodeInput {
            identifier,
            title,
            title_text,
            metadata,
            body,
            backmatter,
        } = input;
        Ok(MockNode {
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
pub struct MockBody {
    pub body_html: String,
    pub links: HashMap<u32, MockLinkInput>,
    pub transclusions: HashMap<u32, MockTransclusionInput>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MockLinkInput {
    pub identifier: String,
    pub metadata: Option<Metadata>,
    pub resolution: Option<MockResolvedLink>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MockResolvedLink {
    pub title: String,
    pub title_text: String,
    pub metadata: Metadata,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MockTransclusionInput {
    pub metadata: Option<Metadata>,
    pub resolution: Option<MockResolvedTransclusion>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MockResolvedTransclusion {
    pub identifier: String,
    pub title: String,
    pub title_text: String,
    pub metadata: Metadata,
    pub body: Box<MockBody>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MockBackmatterNode {
    pub title: String,
    pub title_text: String,
    pub metadata: Metadata,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MockBackmatter {
    pub node: (String, MockBackmatterNode),
    pub contexts: Vec<(String, Option<MockBackmatterNode>)>,
    pub backlinks: Vec<(String, Option<MockBackmatterNode>)>,
    pub outlinks: Vec<(String, Option<MockBackmatterNode>)>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MockNode {
    pub identifier: String,
    pub title: String,
    pub title_text: String,
    pub metadata: Metadata,
    pub body: MockBody,
    pub backmatter: MockBackmatter,
}
