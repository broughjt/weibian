# Datalog and Stage Ordering

## What's stable in Stage 4's graph code

- Transclusion graph (`HashMap<NodeId, Vec<NodeId>>`)
- Reverse transclusion graph (for incremental invalidation)
- Topological sort and cycle detection
- The `rendered_body` field in `NodeEntry`

None of this changes when datalog arrives. Datalog *consumes* the transclusion graph as
ground facts — it doesn't replace it. The topo sort is still needed for rendering order
even with datalog.

## What's actually at risk

**The `Fact` enum and `BackmatterQueryResults` types in `NodeEntry`** — these depend on
which datalog library you pick (`ascent`, hand-rolled, etc.).

**The temptation to write imperative backmatter in Stage 4** following v1's pattern. If
you implement `compute_backlinks`, `compute_contexts`, `compute_transcluded_descendants`
as a "works for now" thing in Stage 4, you'll throw them out in Stage 5. That's the
rework to avoid.

## Recommended approach: a narrow spike before Stage 4

Rather than implementing Stage 5 first or Stage 4 in ignorance, do a focused spike to
answer the specific questions that affect Stage 4's types:

1. **Which datalog approach?** `ascent` (macro-based) or a hand-rolled naive bottom-up
   evaluator. `forester.md` notes the acyclic case is ~200 lines.
2. **What does the `Fact` enum look like** for weibian's ground facts (`transcludes`,
   `links-to`, `has-taxon`, `has-tag`, etc.)?
3. **What does querying look like** — how do you get a `BTreeSet<NodeId>` back for
   backlinks, contexts, references?

Once the interface is known, Stage 4's `NodeEntry` can have the right types (or
right-shaped stubs), and Stage 4 can proceed knowing to leave backmatter entirely for
Stage 5.

## The key discipline in Stage 4

**Don't implement any backmatter.** No backlinks, no contexts, no references. Stage 4
ends with transclusion-resolved body HTML written to disk. Stage 5 adds everything else.
If you hold that line, very little of Stage 4's code will be touched by datalog.

---

## Resources

- https://dl.acm.org/doi/pdf/10.1145/3622840
