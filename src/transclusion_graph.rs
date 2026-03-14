use std::collections::{HashMap, HashSet};

use crate::compiler::NodeId;

/// Graph of transclusion relationships between nodes.
///
/// Built from the NodeStore on demand. Used for topological sort (Stage 4
/// rendering order), cycle detection, and reverse-BFS invalidation in the
/// watch loop.
pub struct TransclusionGraph {
    /// Maps each node to the list of nodes it directly transcludes.
    forward: HashMap<NodeId, Vec<NodeId>>,
    /// Maps each node to the list of nodes that directly transclude it.
    reverse: HashMap<NodeId, Vec<NodeId>>,
}

impl TransclusionGraph {
    /// Builds the graph from an iterator of `(node_id, transclusions)` pairs.
    pub fn build<'a>(
        entries: impl IntoIterator<Item = (&'a NodeId, &'a Vec<NodeId>)>,
    ) -> Self {
        todo!()
    }

    /// Returns a topological ordering of all nodes, with dependencies before
    /// dependents. Returns an error if a transclusion cycle is detected.
    pub fn topo_sort(&self) -> anyhow::Result<Vec<&NodeId>> {
        todo!()
    }

    /// Returns all nodes that transitively transclude `id`, not including
    /// `id` itself.
    pub fn dependents(&self, id: &str) -> HashSet<&NodeId> {
        todo!()
    }
}
