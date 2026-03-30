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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubnodePath(pub Vec<usize>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodePath {
    Primary,
    Subnode(SubnodePath),
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
        target: NodePath,
        update: NodeUpdate,
    },
    AddSubnode {
        parent: NodePath,
        subnode: MockSubnode,
    },
    RemoveSubnode(SubnodePath),
    SetSubnodeTransclude {
        target: SubnodePath,
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
                let (parent_path, index) = path.split_last();
                let subnodes = self.get_subnodes_mut(&parent_path.into());
                subnodes.remove(index);
            }
            FileUpdate::SetSubnodeTransclude { target, transclude } => {
                let subnode = self.get_subnode_mut(&target);
                subnode.transclude = transclude;
            }
        }
    }

    fn get_node_mut(&mut self, path: &NodePath) -> &mut MockNode {
        match path {
            NodePath::Primary => &mut self.primary,
            NodePath::Subnode(p) => &mut self.get_subnode_mut(p).node,
        }
    }

    fn get_subnode_mut(&mut self, path: &SubnodePath) -> &mut MockSubnode {
        let indices = &path.0;
        assert!(!indices.is_empty(), "SubnodePath must not be empty");
        let mut current = &mut self.subnodes[indices[0]];
        for &i in &indices[1..] {
            current = &mut current.subnodes[i];
        }
        current
    }

    fn get_subnodes_mut(&mut self, path: &NodePath) -> &mut Vec<MockSubnode> {
        match path {
            NodePath::Primary => &mut self.subnodes,
            NodePath::Subnode(p) => &mut self.get_subnode_mut(p).subnodes,
        }
    }
}

impl SubnodePath {
    fn split_last(&self) -> (SubnodePath, usize) {
        let (last, rest) = self.0.split_last().expect("SubnodePath must not be empty");
        (SubnodePath(rest.to_vec()), *last)
    }
}

impl From<SubnodePath> for NodePath {
    fn from(path: SubnodePath) -> Self {
        NodePath::Subnode(path)
    }
}

impl MockNode {
    fn apply_update(&mut self, update: NodeUpdate) {
        match update {
            NodeUpdate::Rename(id) => self.identifier = id,
            NodeUpdate::ChangeTitle(title) => self.title = title,
            NodeUpdate::UpdateMetadata(u) => apply_metadata_update(&mut self.metadata, u),
            NodeUpdate::UpdateBody(u) => self.apply_body_update(u),
        }
    }

    fn apply_body_update(&mut self, update: BodyUpdate) {
        match update {
            BodyUpdate::Insert { index, element } => self.body.insert(index, element),
            BodyUpdate::Remove(index) => { self.body.remove(index); }
            BodyUpdate::Update { index, update } => {
                apply_element_update(&mut self.body[index], update);
            }
        }
    }
}

fn apply_metadata_update(metadata: &mut crate::compiler::Metadata, update: MetadataUpdate) {
    match update {
        MetadataUpdate::SetValues { key, values } => { metadata.insert(key, values); }
        MetadataUpdate::RemoveKey(key) => { metadata.remove(&key); }
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

fn apply_element_update(element: &mut MockElement, update: ElementUpdate) {
    match update {
        ElementUpdate::SetText(text) => *element = MockElement::Text(text),
        ElementUpdate::UpdateLink(u) => {
            let MockElement::Link(link) = element else { panic!("element is not a link") };
            match u {
                LinkUpdate::SetTarget(t) => link.target = t,
                LinkUpdate::SetContent(c) => link.content = c,
                LinkUpdate::UpdateMetadata(u) => apply_metadata_update(&mut link.metadata, u),
            }
        }
        ElementUpdate::UpdateTransclusion(u) => {
            let MockElement::Transclusion(t) = element else { panic!("element is not a transclusion") };
            match u {
                TransclusionUpdate::SetTarget(target) => t.target = target,
                TransclusionUpdate::UpdateMetadata(u) => apply_metadata_update(&mut t.metadata, u),
            }
        }
    }
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
   - later: raw target/path resolution for generators

3. property tests
   - scratch vs incremental
   - incremental vs stateless reference

The body is intentionally modeled as a full ordered AST so updates can express
fine-grained changes like:
- adding or removing a single link/transclusion
- editing metadata on one link/transclusion occurrence
- reordering text and inline references

Nested subnodes are addressed structurally:
- `NodePath::Primary`
- `NodePath::Subnode(SubnodePath(vec![i]))` for a top-level subnode
- `NodePath::Subnode(SubnodePath(vec![i, j, k]))` for a deeply nested subnode
*/
