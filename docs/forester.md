# Forester Feature Analysis for Weibian v2

## 1. The Datalog System

This is forester's most architecturally distinctive choice. It's worth understanding concretely before deciding whether to adopt it.

### What forester actually does with datalog

Forester uses the OCaml `datalog` library (bottom-up/BFS evaluation) to maintain a fact database about the entire forest. As each tree is analyzed, the compiler populates the database with ground facts:

- `links-to(A, B)` — A has an explicit link to B
- `transcludes(A, B)` — A includes B
- `has-author(A, P)` — A is authored by person P
- `has-taxon(A, T)` — A has taxon T (e.g. "Definition", "Reference", "Person")
- `has-tag(A, T)` — A has tag T
- `is-node(A)` — A exists in the forest
- `in-host(A, H)` — A lives under domain H

Then a fixed set of **axioms** derives additional relations via fixed-point iteration:

```datalog
% Transitive closure of transclusion
transcludes-tc(X, Y) :- transcludes(X, Y).
transcludes-tc(X, Z) :- transcludes-tc(X, Y), transcludes(Y, Z).

% Reflexive-transitive closure (X includes itself)
transcludes-rtc(X, X) :- is-node(X).
transcludes-rtc(X, Y) :- transcludes-tc(X, Y).

% Taxonomy shortcuts
is-reference(X) :- has-taxon(X, "Reference").
is-person(X)    :- has-taxon(X, "Person").

% References: all bibliography entries reachable via transclusion
references(X, Z) :- transcludes-rtc(X, Y), links-to(Y, Z), is-reference(Z).

% Contributor chains through transclusion
has-direct-contributor(X, Y)   :- has-author(X, Y).
has-indirect-contributor(X, Z) :- transcludes-rtc(X, Y), has-direct-contributor(Y, Z).
```

Each backmatter section is then just a query against this database:

- **Context**: `?X :- transcludes(X, this)` — who includes me
- **Backlinks**: `?X :- links-to(X, this)` — who links to me
- **Related**: `?X :- links-to(this, X), not is-reference(X)` — what I link to, minus bibliography
- **References**: `?X :- references(this, X)` — bibliography entries reachable from me (transitively)
- **Contributions**: `?X :- has-direct-contributor(X, this), is-reference(X)` — reference works this person contributed to

The critical insight: the `references` query uses the transitive closure. A bibliography entry counts as a reference for article A if *any article that A transitively transcludes* links to a "Reference"-taxon node. This means transclusion is semantically "inline content", and the reference list propagates upward. This is non-trivial to compute with imperative code but falls out naturally from the axioms.

### What weibian v1 does instead

`compute_backlinks`, `compute_contexts`, `compute_transcluded_descendants` — all hand-coded reverse-index functions. References are tracked separately via `<wb-cite>` element extraction during rendering. There's no transitive reference propagation: a reference only appears in a note's backmatter if that note directly cites it, not if a transcluded subtree cites it.

### What datalog actually buys you

1. **Transitive reference propagation** — if A transcludes B and B cites [Paper], then A's reference list includes [Paper]. This is the primary motivator in forester and is genuinely hard to express without some form of recursive graph computation.

2. **Contributor inheritance** — if A transcludes B and B has author "Alice", then Alice is an indirect contributor to A. Same story.

3. **User-defined queries in source** — forester lets you embed `\query{?X :- my-rel(X)}` directly in a tree file, and the result renders as a list of matching trees. This enables things like "all definitions that mention concept X" or "all theorems proved by Alice" without hardcoding those queries in the compiler.

4. **User-defined relations in source** — `\datalog{my-rel(X, Y) :- ...}` lets users extend the database with custom relations, then query them. This is how you'd implement, e.g., "all prerequisites of this note".

### The costs

- A datalog runtime dependency (the OCaml `datalog` library is ~2K LOC; a Rust equivalent would be `ascent`, `datalog-rust`, or rolling a naive bottom-up evaluator which is ~200 lines for the simple acyclic case)
- The database is recomputed from scratch on every full build (forester doesn't currently do incremental datalog)
- The vertex model is more complex: forester's datalog facts use `Vertex.t` which is either a URI or inline content, allowing content-addressed facts like `has-taxon(X, "Definition")` where "Definition" is a content vertex rather than a URI

### My read

The datalog system earns its keep for the `references` query (transitive bibliography propagation through transclusion) and for user-defined queries. For a note system with serious mathematical writing (theorems, definitions, bibliography), those two features are genuinely valuable. For simpler use cases the imperative approach in weibian v1 is sufficient.

---

## 2. Transclusion Targets — `Full` vs `Mainmatter` vs `Title` vs `Taxon`

Forester's transclusion has a `content_target` type with four variants:

```ocaml
type content_target =
  | Full of section_flags   (* include the whole article as a section *)
  | Mainmatter              (* body only, no title/metadata *)
  | Title of title_flags    (* just the title text *)
  | Taxon                   (* just the classification label *)
```

Only `Full` and `Mainmatter` add edges to the datalog `transcludes` relation — `Title` and `Taxon` are metadata-only pulls and don't create structural dependencies.

This matters for two reasons:

- It lets you reference a term's label (`\taxon`) or title (`\title`) inline without pulling its full content
- The graph is cleaner: not every reference to another node creates a dependency

Weibian v1's `<wb-transclusion>` is always full-content. Worth considering whether partial transclusion (`#tr-title("id")`, `#tr-taxon("id")`) is useful for your use case.

---

## 3. Designated Parent / Explicit Tree Hierarchy

Forester has a `designated_parent` field in frontmatter: a tree can declare that it "belongs to" another tree. This creates an explicit parent/child relationship beyond what's inferred from transclusion.

The distinction matters: transclusion is a display relationship ("include this content here"), while designated parent is an ontological relationship ("this node is logically a part of that node"). A definition might be transcluded into many proofs but have exactly one designated parent (the section of the textbook it belongs to).

Weibian v1 has no equivalent — hierarchy is inferred purely from transclusion. Your subnode model partially addresses this (a subnode's implicit parent is the file node), but there's no way to declare "this file node belongs to that other file node" without transclusion. Worth considering whether you want a `parent` metadata field in `#node(...)`.

---

## 4. Taxa as a Classification System

Forester's `taxon` is a single content value on a tree that classifies it. The builtin taxa "Reference" and "Person" trigger special datalog rules. But it's open — any string is a valid taxon ("Theorem", "Definition", "Lemma", "Remark", "Conjecture", etc.).

What's interesting architecturally: taxa participate in the datalog system. You can write rules like:

```datalog
is-theorem(X) :- has-taxon(X, "Theorem").
```

...and then query or reason over them. In weibian v1 and in your current design doc, `taxon` is a display metadata field only — it affects how the note looks but doesn't participate in any graph queries. If you add datalog, taxa become query-able automatically.

---

## 5. User-Queryable Relations in Source

This is forester's most distinctive user-facing feature. In a tree file, you can write:

```forester
\query{?X :- links-to(X, self)}
```

and the result is rendered as a list of trees matching the query. You can also define custom relations:

```forester
\datalog{
  prerequisite('self, ?X) :- links-to('self, ?X)
}
```

then later:

```forester
\query{?X :- prerequisite('this-tree, ?X)}
```

The `'` prefix creates a content constant, `@` creates a URI constant, `?` is a variable.

For a Typst-based system, this would look something like:

```typst
#query(rel: "has-taxon", arg2: "Theorem")  // all theorems
#query(rel: "links-to", arg1: "hott-book") // all notes linking to HoTT book
```

The question is whether you want to expose this to authors at all. For a personal notes tool, pre-defined backmatter sections may be enough. But if you ever want "show all definitions related to this concept" as an authoring primitive rather than a display convention, you need some query facility.

---

## 6. `in-host` / Domain Grouping

Forester supports multi-forest setups where trees from different "hosts" (domains) coexist. The `in-host(A, H)` relation groups trees by their URI domain, which enables queries scoped to a single host.

Probably not relevant for weibian v2 which is a single-site tool. Skip.

---

## 7. Import Graph as Separate from Transclusion Graph

Forester maintains two distinct dependency graphs:

1. **Import graph** (`Forest_graph.t`): which tree files import which others (for compilation ordering)
2. **Datalog transclusion graph**: semantic relationships between compiled trees

This exactly mirrors the two-graph design in the architecture doc. Forester validates it as the right separation — the OCaml version has an explicit `build_import_graph` phase before expansion/evaluation.

---

## 8. Backmatter Customization

Forester generates backmatter via `default_backmatter ~uri` which builds a fixed list of sections. Each section is a `Results_of_datalog_query` content node. Users can override the backmatter entirely by writing `\backmatter{...}` in their tree.

More relevantly: each backmatter section is `hidden_when_empty = true`, so empty sections don't render. Weibian v1 has this behavior too. Worth confirming the design handles it.

---

## 9. Sections and Section Flags

Forester's `section` type carries `section_flags`:

```ocaml
type section_flags = {
  toc: bool;                    (* include in table of contents *)
  numbered: bool;               (* show section number *)
  show_metadata: bool;          (* show title/taxon/authors *)
  expanded: bool;               (* whether details is open *)
  hidden_when_empty: bool option;
}
```

These correspond almost exactly to the attributes on weibian v1's `<wb-transclusion>` element (`show-metadata`, `expanded`, `disable-numbering`). The models are equivalent; the difference is whether flags are set at the transclusion site or at the section definition site.

---

## 10. Numbering

Forester has `contextual_number` which assigns sequential numbers to theorems/definitions/lemmas within their parent article. This is the "Theorem 1.3" type numbering you'd expect in mathematical writing.

Weibian v1 has a custom Tera filter `wb_disable_numbering` but no automatic numbering system. If you want "Definition 3" / "Lemma 4" numbering across transcluded content, you'll need some form of this. It's complex because numbering is context-sensitive — the same definition might be "Definition 2.1" in one article and "Definition A.3" in another depending on what transcluded it.

Forester handles this by passing numbering state through the rendering context. Worth flagging this as a future concern rather than a v2 requirement.

---

## Synthesis: What's Worth Adopting

| Feature | Worth it for v2? | Notes |
|---|---|---|
| **Datalog for backmatter queries** | Yes, if you want transitive reference propagation | `references` query is the killer feature; otherwise imperative is fine |
| **User-defined queries in source** | Yes, if you want `#query(...)` as an authoring primitive | Significant complexity; defer to later |
| **Transclusion targets (Full/Title/Taxon)** | Maybe | `#tr-title("id")` is a useful primitive; low implementation cost |
| **Designated parent** | Worth adding as metadata | `parent:` field in `#node(...)`, no implementation needed yet |
| **Taxa in graph queries** | Falls out of datalog for free | Worth noting |
| **Section flags at definition site** | Already in your design | Your `<wb-subnode>` attrs cover this |
| **Multi-phase compilation** | Already in your design | Your build/process split mirrors this |
| **Import graph ≠ transclusion graph** | Already in your design | Your two-graph design is exactly right |
| **Contextual numbering** | Defer | Complex, not a v2 concern |
| **Multi-host/domain grouping** | No | Single-site tool |

The most concrete question for the design doc is: **do you want the `references` backmatter section to propagate transitively through transclusion?** If yes, you need some form of recursive graph computation — datalog is the cleanest way to express it, but you could also write it imperatively (compute the transitive closure of transclusions, then for each node in the closure collect `is-reference` targets). If no, weibian v1's approach is sufficient.

---

## Integrating Datalog into the Process Stage and Watch Loop

### The invalidation model is currently transclusion-only

The design doc's invalidation model says: "the changed nodes plus everything that (transitively) transcludes them." This correctly handles re-rendering the body content — if B changes, anything whose HTML embeds B's HTML needs to be recomputed.

But datalog-backed backmatter is a **second kind of output** that depends on the global fact database, not the transclusion subtree. When node B changes, the backmatter of some other node C can change even if C has no transclusion relationship with B at all:

- A new note is added that links to C → C's **backlinks** section changes, but C's rendered HTML is unchanged
- B changes its taxon to "Reference" → every node that (transitively) transcludes anything that links to B now has its **references** section change — potentially far-reaching
- B adds an author → B's transclusion ancestors all have their **indirect contributors** section change

Under the current design, none of these would trigger a re-render of C, because C isn't in the reverse-transclusion subgraph of B. The output HTML would be stale.

### Two separate invalidation axes

Once you have datalog, the `rendered_html` field in the NodeStore should conceptually split into two independently-invalidatable things:

```
NodeStore entry: {
    raw_html,
    rendered_body,      // transclusion-resolved content — invalidated by reverse-transclusion walk
    rendered_backmatter,// datalog query results — invalidated by query result diffing
    // ... metadata, facts, etc.
}
```

The invalidation rules then become:

- **Invalidate `rendered_body`** when: this node changed, or any transcluded node's `rendered_body` changed (same as current design)
- **Invalidate `rendered_backmatter`** when: any datalog query result for this node changed (requires comparing against cached results)
- **Write output** when: either is invalidated

This separation means you don't have to redo transclusion rendering just because a backlink was added, and vice versa.

### How the datalog database fits into WatchState

The database needs to live at the same level as the `NodeStore` — persistent across watch iterations and owned by the watch loop state. The question is whether you rebuild it fully or maintain it incrementally.

**Full rebuild on every change** is the right starting point. The datalog evaluation itself is fast — for 400–2000 nodes with a few thousand ground facts, bottom-up fixed-point terminates in microseconds. The cost isn't running the datalog; it's knowing *which nodes' backmatter changed* afterward, which requires diffing the new query results against cached old results.

So the flow on a watch event becomes:

```
1. Rebuild dirty source files (existing build stage)
2. Update NodeStore: replace raw_html + facts for changed nodes
3. Rebuild datalog database from scratch (fast)
4. Run all backmatter queries for all nodes, compare against cached results
   → collect set of nodes with changed backmatter
5. Walk reverse-transclusion graph of changed nodes
   → collect set of nodes with stale rendered_body
6. Re-render: union of (4) and (5)
7. Write output
```

Step 4 is the new cost. For 400 nodes × 5 backmatter queries = 2000 query executions per watch event. Still fast — each query is a lookup against an in-memory hash table. But it does mean every watch event touches every node's backmatter queries, even if nothing changed. That's acceptable at realistic scales, and you can short-circuit with a "no facts changed" early exit for the common case where only content changes.

### What the NodeStore entry needs to carry

```rust
struct NodeEntry {
    raw_html: String,
    rendered_body: Option<String>,      // None = needs recompute
    rendered_backmatter: Option<String>,// None = needs recompute

    // For datalog
    contributed_facts: Vec<Fact>,       // facts this node contributes to the DB
    cached_query_results: BackmatterQueryResults, // for change detection

    // Existing
    transclusions: Vec<NodeId>,
    metadata: NodeMetadata,
}

struct BackmatterQueryResults {
    backlinks: BTreeSet<NodeId>,
    contexts: BTreeSet<NodeId>,
    related: BTreeSet<NodeId>,
    references: BTreeSet<NodeId>,
}
```

`contributed_facts` is optional if you rebuild the DB from scratch — you'd just walk all NodeStore entries and re-assert all facts. But it's convenient to have per-node facts for the "what changed" reasoning: if a node's contributed facts are identical to last iteration, you know its backlinks/contexts/related haven't changed for *that node as a source*, though its backmatter as a *target* might still have changed.

### The memory accounting changes slightly

The design doc's memory table should gain a `backmatter HTML` column — it's small (a few KB per node) and separate from `rendered_body`, but with the split model it's worth tracking independently. Also `contributed_facts` and `cached_query_results` are tiny (~hundreds of bytes per node, negligible at these scales).

The eviction note in the design doc still applies but gets more precise: you can evict `rendered_body` and `rendered_backmatter` independently after writing output, since both can be recomputed — body from `raw_html` + transclusion graph, backmatter from the datalog database.
