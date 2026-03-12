use std::collections::{HashMap, HashSet, VecDeque};

use typst::syntax::FileId;

/// Tracks import relationships between files for incremental recompilation.
///
/// Maintains forward edges (mapping a file to its imports) and reverse edges
/// (mapping a file to the files that import it) so that when a file changes,
/// all transitively dependent files can be found efficiently.
#[derive(Default)]
pub struct ImportGraph {
    /// Maps each file to the set of files it imports.
    forward: HashMap<FileId, HashSet<FileId>>,
    /// Maps each file to the set of files that import it.
    reverse: HashMap<FileId, HashSet<FileId>>,
}

impl ImportGraph {
    /// Updates the import edges for `id` given its new set of dependencies.
    ///
    /// Stale edges from a previous compilation are removed and new edges are
    /// added. Both the forward and reverse maps are kept consistent.
    pub fn update(&mut self, id: FileId, new_dependencies: HashSet<FileId>) {
        let old_dependencies = self.forward.remove(&id).unwrap_or_default();

        for &dependency in old_dependencies.difference(&new_dependencies) {
            if let Some(importers) = self.reverse.get_mut(&dependency) {
                importers.remove(&id);
            }
        }

        for &dependency in new_dependencies.difference(&old_dependencies) {
            self.reverse.entry(dependency).or_default().insert(id);
        }

        self.forward.insert(id, new_dependencies);
    }

    /// Removes all graph edges involving `id`.
    ///
    /// Use this when a source file is deleted.
    pub fn remove(&mut self, id: FileId) {
        if let Some(dependencies) = self.forward.remove(&id) {
            for dependency in dependencies {
                if let Some(importers) = self.reverse.get_mut(&dependency) {
                    importers.remove(&id);
                }
            }
        }

        if let Some(importers) = self.reverse.remove(&id) {
            for importer in importers {
                if let Some(dependencies) = self.forward.get_mut(&importer) {
                    dependencies.remove(&id);
                }
            }
        }
    }

    /// Returns an [`HashSet`] over all files that transitively depend on `id`.
    ///
    /// Performs a breadth-first traversal of the reverse edge graph. The
    /// result includes both direct and indirect dependents, but not `id`
    /// itself. Order is unspecified.
    pub fn dependents(&self, id: FileId) -> HashSet<FileId> {
        let mut visited: HashSet<FileId> = HashSet::new();
        let mut queue: VecDeque<FileId> = VecDeque::new();

        // TODO: Reexamine code here, it might be worth ordering things
        // Also, can we toss this first loop? Is it cleaner that way?

        if let Some(direct) = self.reverse.get(&id) {
            for &dep in direct {
                if visited.insert(dep) {
                    queue.push_back(dep);
                }
            }
        }

        while let Some(current) = queue.pop_front() {
            if let Some(importers) = self.reverse.get(&current) {
                for &importer in importers {
                    if visited.insert(importer) {
                        queue.push_back(importer);
                    }
                }
            }
        }

        visited
    }
}
