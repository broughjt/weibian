Now I have a complete picture. Here's the detailed analysis:

---

## Stage 5.5 Analysis

### What "Stage 5.5" would cover

Based on comparing v1 (`backend.rs`) and v2 (`compiler.rs`), the required changes split cleanly into two features:

---

### Part 1: Backmatter

**V1 computes four sections:**

| Section | Query |
|---|---|
| Contexts | nodes that transcluse *me* (reverse transclusion edges) |
| Backlinks | nodes that link to *me* (reverse link edges) |
| Related | nodes *I* link to, taxon ≠ Reference, minus transcluded descendants |
| References | nodes *I* link to, taxon = Reference, minus transcluded descendants |

The "transcluded descendants" exclusion is important and easy to miss. In v1, `compute_transcluded_descendants()` computes the transitive closure of each node's transclusion targets and removes them from Related/References — because transcluded content is already inline, it'd be redundant in the backmatter. This needs porting.

**V1's rendering approach**: backmatter items are rendered as *virtual `<wb-transclusion>` elements* and run through the normal transclusion substitution pass, so backmatter items appear as collapsible expand/collapse blocks. `stage5-plan2.md` proposes a simpler separate `backmatter_template` instead (a plain list of links). This is a conscious visual divergence — easier to implement, slightly different UX.

**Required v2 additions:**
1. `BackmatterCache` struct (`contexts`, `backlinks`, `related`, `references` as `BTreeSet<NodeId>`) + field on `NodeEntry`
2. Pass 3: compute `transcluded_descendants` in topo order, then compute the four sets using existing `self.links` and `self.transclusions` `DiGraphMap`s — already present
3. Pass 4: render via `backmatter_template` into `rendered_backmatter`
4. `backmatter_template` config entry + default `backmatter.html` template
5. `node.html` gets `node.backmatter` variable

**Watch-mode invalidation (the new complexity):**

Backmatter has a cross-node dependency: if node A adds a link to node B, node B's backlinks section changes even though B's body is unchanged. This is a second invalidation axis.

The pattern from `stage5-plan2.md` is correct:
- After Pass 3, diff each node's new `BackmatterCache` against its old one
- Any node whose cache changed → add to render set, but only `rendered_backmatter` needs recomputing (can reuse `rendered_body`)
- **Early exit**: if `self.links.edge_count()` and `self.transclusions.edge_count()` haven't changed and no metadata changed → skip Pass 3+4 entirely (common case: prose-only edit)

The write phase needs to handle two classes of dirty node: (a) body changed — rerender both, (b) only backmatter changed — reuse `rendered_body`, rerender `rendered_backmatter` only.

---

### Part 2: Table of Contents

**Much simpler.** V1's `build_toc()` is a stack-based algorithm that parses `<h1>`–`<h6>` from rendered body HTML and builds a hierarchical `Vec<Heading>` tree with `{level, id, content, disable_numbering, children}`.

**V2 additions:**
1. Port `build_toc()` — `scraper` is already a dependency, selector `h1, h2, h3, h4, h5, h6`, `disable_numbering` detected from `class` attribute (already handled via the `disable_numbering` filter)
2. `Heading` struct (serde-serializable for Jinja)
3. Compute inline during write pass from `rendered_body` — no caching needed, no separate invalidation axis (TOC is derived entirely from the node's own rendered body, which is already correctly invalidated)
4. `node.html` gets `node.toc`

**One difference from v1**: v1 builds TOC from the pre-transclusion body at the link-resolution stage. V2 should build from `rendered_body` (post-transclusion) so headings inside transcluded content are included. This is actually *better* behaviour.

---

### V1→V2 Delta Summary

| Aspect | V1 | V2 after Stage 5.5 |
|---|---|---|
| Backmatter computation | Fresh every run | Cached per node, diffed each cycle |
| Backmatter item rendering | Virtual transclusions (expand/collapse) | Separate template (link list) |
| Transcluded descendant exclusion | `compute_transcluded_descendants()` | Must port |
| Watch invalidation for backmatter | N/A | Diff-based second render axis |
| TOC source | Pre-transclusion body | Post-transclusion `rendered_body` |
| TOC caching | None needed | None needed |

---

### Is Stage 5.5 meaningful?

Yes. The work is well-scoped:
- **Backmatter**: substantial but contained — mostly adding passes to `process()` in `compiler.rs` and a watch-mode diffing step. The hardest part (transclusion cycle detection, render set computation, incremental invalidation) already exists.
- **TOC**: a straightforward port — a self-contained function computing from already-available data.
- **Stage 6 comparison**: Stage 6 requires a datalog evaluator, rules parser, exported-relations mechanism, and ground-facts pipeline. Stage 5.5 is an order of magnitude smaller.

The watch-mode invalidation design in `stage5-plan2.md` is sound. The one thing not yet in that doc is the `transcluded_descendants` exclusion (a v1 detail that would be a subtle correctness gap if omitted).

The analysis is complete. Here's a summary of what Stage 5.5 would require:

---

## Stage 5.5: Backmatter + TOC

**Is it meaningful?** Yes — it's substantially smaller than Stage 6 while delivering the remaining v1 parity features.

### Backmatter

The existing `DiGraphMap` structures for links and transclusions already contain everything needed. The work is:

1. **`BackmatterCache` struct** per node (`contexts`, `backlinks`, `related`, `references` as `BTreeSet<NodeId>`) — new Pass 3 in `process()`
2. **`compute_transcluded_descendants()`** port — v1 computes the transitive closure of each node's transclusion targets and strips those from Related/References (avoids redundancy since transcluded content is already inline). This is the one v1 detail not in `stage5-plan2.md`.
3. **Pass 4: backmatter rendering** via a separate `backmatter_template` (the plan's approach — simpler than v1's virtual-transclusion approach, slightly different visual)
4. **Watch invalidation**: the hard part. Backmatter has a cross-node dependency — node B's backlinks change when node A adds a link to B. The fix: snapshot `edge_count()` before each cycle; if unchanged, skip Passes 3+4 (common prose-only case). If changed, recompute all caches, diff against old, add changed nodes to render set as "backmatter-only dirty" — they reuse `rendered_body` but rerender `rendered_backmatter`.

### Table of Contents

Much simpler — TOC is purely derived from a node's own `rendered_body`, so no second invalidation axis. Port v1's stack-based `build_toc()` (uses `scraper`, already a dep), compute inline during the write pass, pass `node.toc` to `node.html`. One improvement over v1: compute from post-transclusion `rendered_body` so headings inside transcluded content are included.

### Key files
- `src/compiler.rs` — `BackmatterCache`, new passes, invalidation logic
- `src/watch.rs` / `src/build.rs` — graph change detection
- `src/config.rs` — `backmatter_template` config entry
- `templates/backmatter.html` — new template
- `templates/node.html` — `node.backmatter`, `node.toc`

Is this doable in a day of work, or will it be more?

I avoid time estimates — scope varies too much with how the day goes. But I can characterize the complexity of each piece:

  TOC — mechanical. Port a self-contained ~50-line function, add a struct, wire it into the write pass and template. No design decisions.

  Backmatter (straightforward parts) — also mechanical: BackmatterCache struct, Pass 3 computation using existing graph APIs, Pass 4 rendering, config
  entry, templates. The logic is all spelled out.

  Backmatter (watch invalidation) — the only part with real design surface. Two sub-problems:
  - Graph change detection: snapshotting edge counts before/after a compile cycle is simple. The edge-count heuristic is a fast approximation but edge
  deletions + additions could cancel out (same count, different graph). Whether that's acceptable or whether you need a proper structural diff (e.g.
  snapshot collect_edges() as a HashSet) is a judgment call.
  - Two-class render set: the write pass needs to distinguish "body+backmatter dirty" from "backmatter-only dirty" nodes. This requires threading a bit
   of extra state through process() → write. Not complex, but it touches existing code paths rather than adding new ones.

  The backmatter watch invalidation is the only part I'd be hesitant to call "obviously mechanical." The rest is additions, not modifications to
  existing logic.
