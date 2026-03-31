use std::collections::HashMap;
use std::io::Write;

use ecow::EcoVec;
use typst::diag::{SourceDiagnostic, Warned};
use typst::syntax::Span;

use crate::compiler::{Compile, CompileOutput, Metadata};

#[derive(Debug, Clone)]
pub struct MockCompile(pub Warned<Result<MockFile, Vec<SourceDiagnostic>>>);

#[derive(Debug, Clone)]
pub struct MockFile {
    pub primary: MockNode,
    pub subnodes: Vec<MockSubnode>,
}

#[derive(Debug, Clone)]
pub struct MockNode {
    pub identifier: String,
    pub title: String,
    pub metadata: Metadata,
    pub body: Vec<MockElement>,
}

#[derive(Debug, Clone)]
pub struct MockSubnode {
    pub node: MockNode,
    pub transclude: bool,
    pub subnodes: Vec<MockSubnode>,
}

#[derive(Debug, Clone)]
pub enum MockElement {
    Text(String),
    Link(MockLink),
    Transclusion(MockTransclusion),
}

#[derive(Debug, Clone)]
pub struct MockLink {
    pub target: String,
    pub content: Option<String>,
    pub metadata: Metadata,
}

#[derive(Debug, Clone)]
pub struct MockTransclusion {
    pub target: String,
    pub metadata: Metadata,
}

#[derive(Debug, Clone)]
pub enum Event {
    Create(FileId, MockCompile),
    Update(FileId, FileUpdate),
    Replace(FileId, MockCompile),
    Remove(FileId),
}

pub type FileId = u16;

#[derive(Debug, Clone)]
pub enum FileUpdate {
    UpdateNode {
        target: NodePath,
        update: NodeUpdate,
    },
    AddSubnode {
        parent: NodePath,
        subnode: MockSubnode,
    },
    RemoveSubnode(NodePath),
    SetSubnodeTransclude {
        target: NodePath,
        transclude: bool,
    },
}

pub type NodePath = Vec<usize>;

#[derive(Debug, Clone)]
pub enum NodeUpdate {
    UpdateIdentifier(String),
    UpdateTitle(String),
    UpdateMetadata(MetadataUpdate),
    UpdateBody(BodyUpdate),
}

#[derive(Debug, Clone)]
pub enum MetadataUpdate {
    SetValues {
        key: String,
        values: Vec<String>,
    },
    RemoveKey(String),
    InsertValue {
        key: String,
        index: usize,
        value: String,
    },
    RemoveValue {
        key: String,
        index: usize,
    },
}

#[derive(Debug, Clone)]
pub enum LinkUpdate {
    SetTarget(String),
    SetContent(Option<String>),
    UpdateMetadata(MetadataUpdate),
}

#[derive(Debug, Clone)]
pub enum TransclusionUpdate {
    SetTarget(String),
    UpdateMetadata(MetadataUpdate),
}

#[derive(Debug, Clone)]
pub enum BodyUpdate {
    Insert { index: usize, element: MockElement },
    Remove(usize),
    Update { index: usize, update: ElementUpdate },
}

#[derive(Debug, Clone)]
pub enum ElementUpdate {
    SetText(String),
    UpdateLink(LinkUpdate),
    UpdateTransclusion(TransclusionUpdate),
}

impl MockFile {
    pub fn apply_update(&mut self, update: FileUpdate) {
        match update {
            FileUpdate::UpdateNode { target, update } => {
                let node = self.get_node_mut(&target);
                node.apply_update(update);
            }
            FileUpdate::AddSubnode { parent, subnode } => {
                let subnodes = self.get_subnodes_mut(&parent);
                subnodes.push(subnode);
            }
            FileUpdate::RemoveSubnode(path) => {
                let (last, rest) = path.split_last().expect("NodePath must not be empty");
                self.get_subnodes_mut(rest).remove(*last);
            }
            FileUpdate::SetSubnodeTransclude { target, transclude } => {
                let subnode = self.get_subnode_mut(&target);
                subnode.transclude = transclude;
            }
        }
    }

    fn get_node_mut(&mut self, path: &[usize]) -> &mut MockNode {
        if path.is_empty() {
            &mut self.primary
        } else {
            &mut self.get_subnode_mut(path).node
        }
    }

    fn get_subnode_mut(&mut self, path: &[usize]) -> &mut MockSubnode {
        assert!(!path.is_empty(), "NodePath must not be empty");
        path[1..]
            .iter()
            .fold(&mut self.subnodes[path[0]], |s, &i| &mut s.subnodes[i])
    }

    fn get_subnodes_mut(&mut self, path: &[usize]) -> &mut Vec<MockSubnode> {
        if path.is_empty() {
            &mut self.subnodes
        } else {
            &mut self.get_subnode_mut(path).subnodes
        }
    }
}

impl MockNode {
    fn apply_update(&mut self, update: NodeUpdate) {
        match update {
            NodeUpdate::UpdateIdentifier(id) => self.identifier = id,
            NodeUpdate::UpdateTitle(title) => self.title = title,
            NodeUpdate::UpdateMetadata(u) => apply_metadata_update(&mut self.metadata, u),
            NodeUpdate::UpdateBody(u) => self.apply_body_update(u),
        }
    }

    fn apply_body_update(&mut self, update: BodyUpdate) {
        match update {
            BodyUpdate::Insert { index, element } => self.body.insert(index, element),
            BodyUpdate::Remove(index) => {
                self.body.remove(index);
            }
            BodyUpdate::Update { index, update } => {
                self.body[index].apply_update(update);
            }
        }
    }
}

fn apply_metadata_update(metadata: &mut crate::compiler::Metadata, update: MetadataUpdate) {
    match update {
        MetadataUpdate::SetValues { key, values } => {
            metadata.insert(key, values);
        }
        MetadataUpdate::RemoveKey(key) => {
            metadata.remove(&key);
        }
        MetadataUpdate::InsertValue { key, index, value } => {
            metadata.entry(key).or_default().insert(index, value);
        }
        MetadataUpdate::RemoveValue { key, index } => {
            if let Some(values) = metadata.get_mut(&key) {
                values.remove(index);
            }
        }
    }
}

impl MockElement {
    fn apply_update(&mut self, update: ElementUpdate) {
        match update {
            ElementUpdate::SetText(text) => *self = MockElement::Text(text),
            ElementUpdate::UpdateLink(u) => {
                let MockElement::Link(link) = self else {
                    panic!("element is not a link")
                };
                match u {
                    LinkUpdate::SetTarget(t) => link.target = t,
                    LinkUpdate::SetContent(c) => link.content = c,
                    LinkUpdate::UpdateMetadata(u) => apply_metadata_update(&mut link.metadata, u),
                }
            }
            ElementUpdate::UpdateTransclusion(u) => {
                let MockElement::Transclusion(t) = self else {
                    panic!("element is not a transclusion")
                };
                match u {
                    TransclusionUpdate::SetTarget(target) => t.target = target,
                    TransclusionUpdate::UpdateMetadata(u) => {
                        apply_metadata_update(&mut t.metadata, u)
                    }
                }
            }
        }
    }
}

impl Compile for MockCompile {
    fn compile(self) -> Warned<Result<CompileOutput, EcoVec<SourceDiagnostic>>> {
        self.0.map(|result| {
            result
                .map(|file| MockFile::render(&file))
                .map_err(EcoVec::from)
        })
    }
}

impl MockFile {
    pub fn render(&self) -> CompileOutput {
        let mut transclusion_counter = 0u32;
        let mut link_counter = 0u32;
        let mut spans = HashMap::new();
        let mut metadata = HashMap::new();
        let mut transclusion_metadata = HashMap::new();
        let mut link_metadata = HashMap::new();

        let mut html = render_node(
            &self.primary,
            &mut transclusion_counter,
            &mut link_counter,
            &mut spans,
            &mut metadata,
            &mut transclusion_metadata,
            &mut link_metadata,
        );
        for subnode in &self.subnodes {
            html.push_str(&render_subnode(
                subnode,
                &mut transclusion_counter,
                &mut link_counter,
                &mut spans,
                &mut metadata,
                &mut transclusion_metadata,
                &mut link_metadata,
            ));
        }

        CompileOutput {
            html,
            spans,
            metadata,
            transclusion_metadata,
            link_metadata,
            errors: EcoVec::new(),
        }
    }
}

fn render_node(
    node: &MockNode,
    transclusion_counter: &mut u32,
    link_counter: &mut u32,
    spans: &mut HashMap<String, Span>,
    metadata: &mut HashMap<String, Metadata>,
    transclusion_metadata: &mut HashMap<u32, Metadata>,
    link_metadata: &mut HashMap<u32, Metadata>,
) -> String {
    spans.insert(node.identifier.clone(), Span::detached());
    if !node.metadata.is_empty() {
        metadata.insert(node.identifier.clone(), node.metadata.clone());
    }

    let mut html = format!(r#"<wb-node identifier="{}">"#, node.identifier);
    html.push_str(&format!("<wb-title>{}</wb-title>", node.title));
    html.push_str(&render_body(
        &node.body,
        transclusion_counter,
        link_counter,
        transclusion_metadata,
        link_metadata,
    ));
    html.push_str("</wb-node>");
    html
}

fn render_subnode(
    subnode: &MockSubnode,
    transclusion_counter: &mut u32,
    link_counter: &mut u32,
    spans: &mut HashMap<String, Span>,
    metadata: &mut HashMap<String, Metadata>,
    transclusion_metadata: &mut HashMap<u32, Metadata>,
    link_metadata: &mut HashMap<u32, Metadata>,
) -> String {
    spans.insert(subnode.node.identifier.clone(), Span::detached());
    if !subnode.node.metadata.is_empty() {
        metadata.insert(
            subnode.node.identifier.clone(),
            subnode.node.metadata.clone(),
        );
    }

    let transclude = if subnode.transclude { "true" } else { "false" };
    let mut html = format!(
        r#"<wb-subnode identifier="{}" transclude="{transclude}">"#,
        subnode.node.identifier,
    );
    html.push_str(&format!("<wb-title>{}</wb-title>", subnode.node.title));
    html.push_str(&render_body(
        &subnode.node.body,
        transclusion_counter,
        link_counter,
        transclusion_metadata,
        link_metadata,
    ));
    for child in &subnode.subnodes {
        html.push_str(&render_subnode(
            child,
            transclusion_counter,
            link_counter,
            spans,
            metadata,
            transclusion_metadata,
            link_metadata,
        ));
    }
    html.push_str("</wb-subnode>");
    html
}

fn render_body(
    body: &[MockElement],
    transclusion_counter: &mut u32,
    link_counter: &mut u32,
    transclusion_metadata: &mut HashMap<u32, Metadata>,
    link_metadata: &mut HashMap<u32, Metadata>,
) -> String {
    let mut html = String::new();
    for element in body {
        match element {
            MockElement::Text(text) => html.push_str(&format!("<p>{text}</p>")),
            MockElement::Link(link) => {
                let c = *link_counter;
                *link_counter += 1;
                if !link.metadata.is_empty() {
                    link_metadata.insert(c, link.metadata.clone());
                }
                let content = link.content.as_deref().unwrap_or("");
                html.push_str(&format!(
                    r#"<a href="wb:{}" data-counter="{c}">{content}</a>"#,
                    link.target,
                ));
            }
            MockElement::Transclusion(t) => {
                let c = *transclusion_counter;
                *transclusion_counter += 1;
                if !t.metadata.is_empty() {
                    transclusion_metadata.insert(c, t.metadata.clone());
                }
                html.push_str(&format!(
                    r#"<wb-transclude identifier="{}" counter="{c}"></wb-transclude>"#,
                    t.target,
                ));
            }
        }
    }
    html
}

/*
Next pieces to reintroduce incrementally:

1. event generation
   - correct `MockFile` generation
   - small focused `...Update` generation
   - later: raw target/path resolution for generators

2. property tests
   - scratch vs incremental
   - incremental vs stateless reference

The body is intentionally modeled as a full ordered AST so updates can express
fine-grained changes like:
- adding or removing a single link/transclusion
- editing metadata on one link/transclusion occurrence
- reordering text and inline references

Nested subnodes are addressed structurally:
- `NodePath(vec![])` for the primary node
- `NodePath(vec![i])` for a top-level subnode
- `NodePath(vec![i, j, k])` for a deeply nested subnode
*/
