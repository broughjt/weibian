use typst::diag::Warned;

use crate::compiler::Metadata;

pub type MockCompile = Warned<Result<MockFile, Vec<String>>>;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeTarget {
    Primary,
    Subnode(usize),
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
pub enum ElementUpdate {
    SetText(String),
    UpdateLink(LinkUpdate),
    UpdateTransclusion(TransclusionUpdate),
}

#[derive(Debug, Clone)]
pub enum BodyUpdate {
    Insert { index: usize, element: MockElement },
    Remove(usize),
    Update { index: usize, update: ElementUpdate },
}

#[derive(Debug, Clone)]
pub enum NodeUpdate {
    Rename(String),
    ChangeTitle(String),
    UpdateMetadata(MetadataUpdate),
    UpdateBody(BodyUpdate),
}

#[derive(Debug, Clone)]
pub enum FileUpdate {
    UpdateNode {
        target: NodeTarget,
        update: NodeUpdate,
    },
    AddSubnode(MockSubnode),
    RemoveSubnode(usize),
    SetSubnodeTransclude {
        index: usize,
        transclude: bool,
    },
}

#[derive(Debug, Clone)]
pub enum Event {
    Create(u16, MockCompile),
    Update(u16, FileUpdate),
    Replace(u16, MockCompile),
    Remove(u16),
}

/*
Next pieces to reintroduce incrementally:

1. `impl MockFile`
   - `apply_update`
   - identifier normalization
   - html lowering / `Compile` impl support

2. event generation
   - correct `MockFile` generation
   - small focused `...Update` generation
   - later: raw target/index resolution for generators

3. property tests
   - scratch vs incremental
   - incremental vs stateless reference

The body is intentionally modeled as a full ordered AST so updates can express
fine-grained changes like:
- adding or removing a single link/transclusion
- editing metadata on one link/transclusion occurrence
- reordering text and inline references
*/
