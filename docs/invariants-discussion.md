Another question, is the existing watch invalidation in Compiler naive? Am I writing code that will be difficult to maintain in the future because
of the amount of state invariants I have to keep track of? Can you give an opinion on the sophistication in compiler.rs?

Having read all 1175 lines carefully, here's my honest assessment:

**The invalidation design is not naive.** The render set computation is precise and formally justified. The comment at lines 202–236 gives the actual set algebra:

> $D^* \cup (R^* \setminus R) = (D^* \cup R^*) \setminus R$

...and explains *why* the simplification holds ($D^* \cap R = \emptyset$ because removed nodes have no outgoing edges in the forward graph, hence no incoming edges in the reversed graph, hence BFS from $D$ cannot reach them). This isn't just ad-hoc bookkeeping — it's stated as a theorem with a proof. The `assert!(self.dirty.is_disjoint(&self.removed))` at line 238 is a runtime guard that enforces the invariant the proof depends on.

The Tarjan SCC usage for cycle detection (lines 269–333) is similarly non-trivial. It exploits Tarjan's output being in reverse topological order (leaves first) to propagate unrenderability in a single pass without a second BFS. The reasoning is spelled out in the comment.

**Where the complexity does live** is in `Compiler`'s state surface. Nine fields that must stay mutually consistent:

- `file_to_nodes` ↔ `node_to_file` are mirror maps — keeping them in sync is `remove()`'s responsibility
- `dirty ∩ removed = ∅` — maintained by `remove()` calling `dirty.remove()` and `compile()` calling `removed.remove()` on de-orphaned nodes
- Removed nodes must have no outgoing edges in either graph — maintained by `clear_outgoing` in `remove()`
- `rendered_body` must be non-None for any renderable node after Pass 2 — maintained by the topological render order

These are real invariants that must all hold simultaneously, and violating any one of them silently produces wrong output or a panic. The panics marked `"bug:"` are the places where invariant violations would surface.

**For Stage 5.5 specifically**, the good news is that backmatter invalidation can be kept *entirely local to `process()`* — it doesn't need to add new persistent fields to `Compiler`. The backmatter dirty set is a derived computation:

1. Snapshot graph structure before the compile phase
2. After Pass 3 (new), diff `BackmatterCache` values against previous
3. Produce `backmatter_dirty` as a local `HashSet<NodeId>` within `process()`
4. The write pass does `render ∪ backmatter_dirty` for output, distinguishing whether a node is body-dirty (re-render both) or backmatter-only-dirty (reuse `rendered_body`, re-render `rendered_backmatter`)

The one new persistent field on `NodeEntry` is `backmatter_cache: Option<BackmatterCache>`, which is read/written only in Pass 3 and has no interaction with the existing invariants around `dirty`/`removed`/`rendered_body`.

So: the existing code is sophisticated but well-reasoned, and Stage 5.5 doesn't need to add any new cross-field invariants to `Compiler` itself. The backmatter state is local to `process()` or confined to a single new `NodeEntry` field. It's a meaningful addition but not one that multiplies the maintenance surface.
