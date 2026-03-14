# Transclusion Resolution and Backmatter — Architecture

## Data layer overview

Three distinct layers, from persistent to ephemeral:

1. **`NodeStore`** — `HashMap<NodeId, NodeEntry>`, persistent across watch iterations.
   Primary source of truth for all node data.
2. **`TransclusionGraph`** — `HashMap`-based forward and reverse maps, derived from the
   NodeStore on demand. Used for topological sort, cycle detection, and reverse-BFS
   invalidation in the watch loop.
3. **Datafrog pass** — an ephemeral computation that reads NodeStore fields to build
   `Relation` inputs, runs the fixed-point, and stores results back into NodeStore entries.
   Datafrog never owns your data.

---

## NodeEntry

Replace the current `HashMap<NodeId, (String, Span)>` with `HashMap<NodeId, NodeEntry>`:

```rust
struct NodeEntry {
    raw_html: String,

    // Extracted during the process stage — feeds both graphs
    transclusions: Vec<NodeId>,   // <wb-transclude> targets → transclusion graph + datafrog
    links: Vec<NodeId>,           // <wb-internal-link> targets → datafrog links_to
    metadata: NodeMetadata,       // taxon, tags, authors → datafrog has_taxon etc.

    // Filled in during rendering — the Stage 4/5 split
    rendered_body: Option<String>,        // Stage 4: transclusion-resolved HTML
    rendered_backmatter: Option<String>,  // Stage 5: datafrog query results rendered

    // Stage 5: cached query results for change detection in the watch loop
    backmatter_cache: Option<BackmatterCache>,
}

struct NodeMetadata {
    taxon: Option<String>,
    tags: Vec<String>,
    // authors, date, etc. as needed
}

struct BackmatterCache {
    backlinks: BTreeSet<NodeId>,
    contexts: BTreeSet<NodeId>,
    references: BTreeSet<NodeId>,
    related: BTreeSet<NodeId>,
}
```

The `Span` stays inside `Compiler` — it is a compile-time concern for diagnostics, not
needed during rendering.

---

## How datafrog fits in

Facts are not a separate enum or data structure. They are derived directly from NodeStore
fields when the datafrog pass runs:

```rust
// Iterate NodeStore entries and collect fields into Relation inputs
let transcludes: Relation<(u32, u32)> = node_store.values()
    .flat_map(|e| e.transclusions.iter().map(|&t| (e.numeric_id, t)))
    .collect();

let links_to: Relation<(u32, u32)> = node_store.values()
    .flat_map(|e| e.links.iter().map(|&t| (e.numeric_id, t)))
    .collect();

let is_reference: Relation<(u32, ())> = node_store.values()
    .filter(|e| e.metadata.taxon.as_deref() == Some("Reference"))
    .map(|e| (e.numeric_id, ()))
    .collect();
```

String NodeIds are interned to `u32` at the start of the datafrog pass (a
`HashMap<NodeId, u32>` built once per pass). After the fixed-point runs, results are
stored back into the NodeStore as `BackmatterCache` entries.

The watch loop diffs new cache values against old ones to find nodes whose backmatter
changed and need re-rendering — this is the second invalidation axis described in
`forester.md`.

---

## TransclusionGraph

Mirrors the existing `ImportGraph`. Built by walking the NodeStore's `transclusions`
fields:

```rust
struct TransclusionGraph {
    forward: HashMap<NodeId, Vec<NodeId>>,  // who transcludes whom
    reverse: HashMap<NodeId, Vec<NodeId>>,  // who is transcluded by whom
}
```

Exposes:
- Topological sort (port of v1's `topo_sort_transclusions`)
- Cycle detection with a clear error
- `dependents(id)` — reverse-BFS to find all nodes that (transitively) transclude a
  given node, used for Stage 4 invalidation in the watch loop

---

## Implementation steps

**Step 1 — Define `NodeEntry` and migrate NodeStore.**
Replace `HashMap<NodeId, (String, Span)>` with `HashMap<NodeId, NodeEntry>`. Populate
`raw_html` from what is currently stored. Leave `transclusions`, `links`, `metadata`
stubbed (empty). Leave `rendered_body`, `rendered_backmatter`, `backmatter_cache` as
`None`. This unblocks everything downstream.

**Step 2 — Enrich extraction.**
During `extract()`, scan each node's HTML for `<wb-transclude>` identifier attributes
and `<wb-internal-link>` targets to populate `entry.transclusions` and `entry.links`.
Scan `<wb-node>` / `<wb-subnode>` element attributes for `entry.metadata`.

**Step 3 — Add `TransclusionGraph`.**
Walk the NodeStore, build forward and reverse maps, add topological sort and cycle
detection. Port directly from v1's `topo_sort_transclusions` in `backend.rs`.

**Step 4 — Transclusion resolution (Stage 4).**
In `process()`, topo-sort the nodes and render in order, substituting `<wb-transclude>`
elements with the already-rendered `rendered_body` of the target. Fill in
`entry.rendered_body`. Do not implement any backmatter here — stop at body rendering.

**Step 5 — Datafrog pass and backmatter (Stage 5).**
After the NodeStore is fully populated with `transclusions`, `links`, and `metadata`,
run the datafrog computation (the `datalog2.rs` spike is the template for this). Store
results in `backmatter_cache`. Render backmatter into `rendered_backmatter`. Add the
second invalidation axis to the watch loop.
