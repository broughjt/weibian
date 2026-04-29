# Stage 5 — Backmatter Invalidation

## The problem

The boundary between Stage 4 and Stage 5 is fuzzy because they share the question: **which nodes need re-rendering after a change?**

Stage 4 (transclusion resolution) has a clean answer: a node needs re-rendering if its own content changed, or if any transclusion descendant changed — the inlined content would differ. This is a reverse-BFS from dirty nodes in the transclusion graph, analogous to file-dependency invalidation in the watch loop.

Stage 5 (backmatter) introduces a harder problem. A node's backmatter depends on *incoming* edges — who links to it, who transcludes it — and potentially the rendered content of those referencing nodes. Two distinct cases:

1. **The backlink set changes.** Node A adds or drops a link to B. B needs re-rendering regardless of whether B itself changed. This is detectable from the links graph delta.

2. **A backlink's content changes.** A is still linking to B, but A's title or content changed. If B's backmatter template renders A's title, B needs re-rendering too. This is where it gets recursive and expensive.

## The "pure function" framing

It's useful to view rendering as a pure function:

```
render(node_content, query_inputs) → html
```

where `query_inputs` is the set of data the template is allowed to query while rendering backmatter (backlinks, transclusion parents, metadata of referencing nodes, etc.). If any input changes, the node needs re-rendering.

The right invalidation strategy falls directly out of what queries the template is allowed to make:

- If the template can only query the **set of node IDs** linking here, invalidation is cheap: re-render when the incoming edge set changes (detectable from the links graph delta).
- If the template can query the **rendered content** of those linking nodes, you need a second propagation pass through the links graph — potentially re-rendering a large fraction of the graph on each change.

## Stage 4 render order

For Stage 4, the render order is determined as follows:

1. Toposort the full transclusion graph (also serving as cycle detection).
2. Compute the re-render set via reverse-BFS from dirty nodes.
3. Filter the toposort result to nodes in the re-render set.
4. Render in that filtered order (children before parents, since toposort returns each node before its successors and we want the reverse).

Isolated nodes (not in the transclusion graph at all) need to be handled separately: render them directly if dirty.

Toposorting just the re-render subgraph would also be correct — a subgraph of an acyclic graph is acyclic, and its topological order is valid. The reverse-BFS already visits the subgraph at O(k + e_k), and a restricted DFS toposort costs the same. The potential optimisation is therefore:

- **Graph structure unchanged** (common case: prose edited, no transclusions added/removed): use a cached toposort order, filter to re-render set — O(k).
- **Graph structure changed**: re-run toposort, update cache, filter — O(V + E).

For now, the simpler full-graph toposort + filter approach is used. For a typical notes corpus V + E is small enough that this is unlikely to matter.

## How to proceed

Implement Stage 4 first, without backmatter, using simple reverse-BFS dirty propagation through the transclusion graph. Defer the backmatter invalidation question until the template query API is designed — the details of what queries are allowed will determine the correct and minimal invalidation strategy.
