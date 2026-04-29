use std::{collections::HashMap, fmt::Write};

use typst_syntax::Span;

use crate::compiler::{Metadata, NodeEntry, extract::NodeOutput};

#[derive(Debug, Clone)]
pub struct MockNode {
    pub identifier: MockNodeIdentifier,
    pub title: String,
    pub body: String,
    pub span: Span,
    pub metadata: Metadata,
    pub transclusions: Vec<MockTransclusion>,
    pub links: Vec<MockLink>,
}

#[derive(Debug, Clone)]
pub struct MockTransclusion {
    pub target: MockNodeIdentifier,
    pub metadata: Metadata,
}

#[derive(Debug, Clone)]
pub struct MockLink {
    pub target: MockNodeIdentifier,
    pub content: Option<String>,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MockNodeIdentifier(pub u32);

impl From<MockNode> for NodeOutput {
    fn from(node: MockNode) -> Self {
        let mut body_html = String::new();
        let mut transclusion_metadata: HashMap<u32, Metadata> = HashMap::new();
        let mut link_metadata: HashMap<u32, Metadata> = HashMap::new();
        let mut transclusions = Vec::with_capacity(node.transclusions.len());
        let mut links = Vec::with_capacity(node.links.len());

        if !node.body.is_empty() {
            write!(body_html, "<p>{}</p>", node.body).unwrap();
        }

        for (counter, transclusion) in node.transclusions.into_iter().enumerate() {
            let counter = counter as u32;
            write!(
                body_html,
                r#"<wb-transclude identifier="{}" counter="{counter}"></wb-transclude>"#,
                transclusion.target.0,
            )
            .unwrap();
            if !transclusion.metadata.is_empty() {
                transclusion_metadata.insert(counter, transclusion.metadata);
            }
            transclusions.push(transclusion.target.0.to_string());
        }

        for (counter, link) in node.links.into_iter().enumerate() {
            let counter = counter as u32;
            let content = link.content.as_deref().unwrap_or_default();
            write!(
                body_html,
                r#"<a href="wb:{}" data-counter="{counter}">{content}</a>"#,
                link.target.0,
            )
            .unwrap();
            if !link.metadata.is_empty() {
                link_metadata.insert(counter, link.metadata);
            }
            links.push(link.target.0.to_string());
        }

        let entry = NodeEntry {
            body_html,
            title: node.title.clone(),
            title_text: node.title,
            span: node.span,
            node_metadata: node.metadata,
            transclusion_metadata,
            link_metadata,
        };

        NodeOutput {
            entry,
            transclusions,
            links,
        }
    }
}
