# Stage 4 — Transclusion Resolution: `process()` Design

This document covers the validation checks and failure modes that `process()` must handle before rendering, and the render loop itself.

---

## Re-render set computation

The starting points for the reverse BFS are **both** `dirty` and `removed`:

- `dirty`: nodes whose source content changed this cycle — already handled.
- `removed`: nodes that were deleted. Their transclusion ancestors still have edges pointing to them and will produce dangling content; those ancestors need re-rendering. Only the ancestors enter the re-render set — the deleted nodes themselves get output-file deletes, not re-renders.

This is a bug in the current stub: `remove()` does not seed the reverse BFS, so ancestors of deleted nodes are silently left stale.

---

## Cycle detection

Cycles are **errors**. Rendering a node in a cycle requires rendering itself first — there is no valid output to produce, so a warning implying a degraded result would be misleading.

### Scope of failure

Only the cyclic nodes and their transclusion ancestors need to be skipped; the rest of the graph renders normally. The precise set is: all SCCs of size > 1 (or with a self-loop), plus all nodes reachable from those SCCs in the forward direction of the transclusion graph. In the condensation DAG (the DAG of SCCs), this is the cyclic SCCs and everything downstream of them.

### Graph theory note

The relevant concept is the **strongly connected component (SCC)**, not "connected component" (which is an undirected term). petgraph's `condensation()` computes the full SCC decomposition via Kosaraju's algorithm and is the right tool here.

### Diagnostics

Report the **full cycle path** (`cycle: 000S → 000M → 000S`), not just a single node. petgraph's `toposort` only returns one node from the cycle; reconstructing the path requires either `condensation()` or a DFS through the SCC. A single node name is not actionable for the author.

---

## Dangling transclusions

Dangling transclusions are **warnings**. In watch mode, transclusion targets are frequently absent while the author is actively writing — referencing a node before creating it is normal. Treating this as a fatal error would make the watch loop painful to use.

Render the node with an empty slot (or a visible `<wb-missing identifier="...">` placeholder) and continue. The warning tells the author what is unresolved.

A deleted node that is still referenced is a special case of a dangling transclusion. The diagnostic can be more specific: "node `foo` was deleted this cycle" vs. "node `foo` has never been defined."

---

## Dangling links

Dangling links are **warnings**, softer than dangling transclusions. A link has no effect on render order or content substitution — the `<a href="wb:foo">` element is already present in the raw HTML, it just points to a nonexistent destination. Consider only reporting these under a `--strict` flag rather than by default.

---

## Isolated nodes

Nodes not present in the transclusion graph at all are invisible to the toposort and reverse BFS. They must be handled with a separate pass: if an isolated node is in `dirty`, render it directly. This is noted in `stage5.md` and must not be forgotten in the rendering loop.

---

## Render loop outline

```
1. Compute re-render set
   - Reverse BFS from dirty nodes (content changed)
   - Reverse BFS from removed nodes (ancestors of deleted nodes)

2. Cycle detection via SCC decomposition
   - Identify cyclic SCCs
   - Emit full-path cycle errors
   - Mark cyclic SCCs + their forward-reachable ancestors as unrenderable
   - Remove unrenderable nodes from the re-render set

3. Dangling transclusion check (over re-render set)
   - Emit warnings
   - Plan to render with empty/placeholder slots

4. Dangling link check (over re-render set)
   - Emit warnings

5. Determine render order
   - Toposort the full transclusion graph (excluding unrenderable nodes)
   - Filter to re-render set
   - Render leaves-first (reverse topological order)

6. Handle isolated dirty nodes
   - Render directly, outside the toposort

7. Apply output plan
   - Write rendered HTML for re-rendered nodes
   - Delete output files for removed nodes
```
