use std::collections::HashMap;

use super::super::Metadata;
use super::super::render::{BackmatterInput, BackmatterNode, BodyInput, NodeInput, Render};

pub struct MockRenderer;

impl Render for MockRenderer {
    type Body = MockBody;
    type Backmatter = MockBackmatter;
    type Node = MockNode;

    fn render_body(&self, input: BodyInput<'_, MockBody>) -> anyhow::Result<MockBody> {
        Ok(MockBody {
            body_html: input.body_html.to_owned(),
            links: input
                .links
                .into_iter()
                .map(|(k, v)| {
                    (
                        k,
                        MockLinkInput {
                            identifier: v.identifier.to_owned(),
                            metadata: v.metadata.cloned(),
                            resolution: v.resolution.map(|r| MockResolvedLink {
                                title: r.title.to_owned(),
                                title_text: r.title_text.to_owned(),
                                metadata: r.metadata.clone(),
                            }),
                        },
                    )
                })
                .collect(),
            transclusions: input
                .transclusions
                .into_iter()
                .map(|(k, v)| {
                    (
                        k,
                        MockTransclusionInput {
                            metadata: v.metadata.cloned(),
                            resolution: v.resolution.map(|r| MockResolvedTransclusion {
                                identifier: r.identifier.to_owned(),
                                title: r.title.to_owned(),
                                title_text: r.title_text.to_owned(),
                                metadata: r.metadata.clone(),
                                body: Box::new(r.body.clone()),
                            }),
                        },
                    )
                })
                .collect(),
        })
    }

    fn render_backmatter(&self, input: BackmatterInput<'_>) -> anyhow::Result<MockBackmatter> {
        fn to_mock_node(n: BackmatterNode<'_>) -> MockBackmatterNode {
            MockBackmatterNode {
                title: n.title.to_owned(),
                title_text: n.title_text.to_owned(),
                metadata: n.metadata.clone(),
            }
        }

        Ok(MockBackmatter {
            node: (input.node.0, to_mock_node(input.node.1)),
            contexts: input
                .contexts
                .into_iter()
                .map(|(id, opt)| (id, opt.map(to_mock_node)))
                .collect(),
            backlinks: input
                .backlinks
                .into_iter()
                .map(|(id, opt)| (id, opt.map(to_mock_node)))
                .collect(),
            outlinks: input
                .outlinks
                .into_iter()
                .map(|(id, opt)| (id, opt.map(to_mock_node)))
                .collect(),
        })
    }

    fn render_node(
        &self,
        input: NodeInput<'_, MockBody, MockBackmatter>,
    ) -> anyhow::Result<MockNode> {
        Ok(MockNode {
            identifier: input.identifier,
            title: input.title.to_owned(),
            title_text: input.title_text.to_owned(),
            metadata: input.metadata.clone(),
            body: input.body.clone(),
            backmatter: input.backmatter.clone(),
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
