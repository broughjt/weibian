# Stage 6 — Datalog Evaluator and Generic Backmatter

Stage 6 replaces Stage 5's hardcoded backmatter queries with a general
datalog evaluator driven by a user-supplied rules file. The milestone is
moving all semantic knowledge out of the compiler and into user-controlled
rules and templates.

See `metadata.md` for the full design of the generic metadata mechanism and
`egglog.md` for the analysis of why existing libraries were ruled out in
favour of a custom evaluator.

---

## What changes from Stage 5

**In the compiler:**
- `BackmatterCache` becomes `HashMap<String, BTreeSet<NodeId>>` keyed by
  relation name, replacing the named-field struct
- The hardcoded backmatter computation pass is replaced by the datalog pass
- The watch loop diffing generalises over the HashMap keys rather than named
  fields — the structure is otherwise identical

**New infrastructure:**
- A rules file parser (standard Datalog syntax)
- A bottom-up evaluator with semi-naive evaluation for recursive rules and
  stratified negation for complement filters
- Ground fact assembly from `NodeEntry` data
- Exported relation resolution (IDs → node objects for templates)

**In user-land:**
- `weibian.toml` gains `[datalog]` with `rules` and `export` keys
- The rules file replaces any hardcoded query logic
- The backmatter template iterates exported relations by name rather than
  fixed section names

---

## Ground facts

The compiler contributes three relation types as ground facts. Nothing else:

```
transcludes(X, Y)            -- from the transclusion DiGraphMap
links_to(X, Y)               -- from the links DiGraphMap
node_meta(X, "key", "value") -- one fact per entry in NodeEntry.metadata
```

`NodeId(u32)` is used directly as the relation element type — no new
interning needed. String keys and values from `metadata` are interned to `u32`
by the evaluator at the start of each pass.

---

## The evaluator

A custom bottom-up evaluator, not an external library. Approximately 300–400
lines covering:

- **Parser**: standard Datalog syntax (`:- `, comma-separated body atoms,
  `not` prefix for negation, string literals for constants)
- **Stratification**: detect negation dependencies between rules, compute
  evaluation strata, error on unstratifiable programs (cycles through negation)
- **Evaluation**: naive bottom-up for non-recursive strata; semi-naive
  (delta-based) for recursive strata to avoid redundant recomputation of the
  transitive closure
- **Primitive constraints**: `=` and `!=` for equality guards in rule bodies

The evaluator runs over small relations (hundreds of nodes, a few thousand
edges) so the performance bar is low. Correctness and simplicity matter more
than throughput.

---

## Example rules file

The rules that were hardcoded in Stage 5 become user-written:

```datalog
is_reference(X) :- node_meta(X, "taxon", "Reference").
is_person(X)    :- node_meta(X, "taxon", "Person").

backlinks(X, Y) :- links_to(Y, X).
contexts(X, Y)  :- transcludes(Y, X).
related(X, Y)   :- links_to(X, Y), not is_reference(Y).

transcludes_tc(X, Z)  :- transcludes(X, Y), transcludes_tc(Y, Z).
transcludes_rtc(X, X) :- transcludes(X, _).
transcludes_rtc(X, Y) :- transcludes_tc(X, Y).

references(X, Z) :- transcludes_rtc(X, Y), links_to(Y, Z), is_reference(Z).
```

This is also where transitive reference propagation is unlocked for the first
time — Stage 5's `references` was direct links only.

---

## Exported relations

`weibian.toml`:

```toml
[datalog]
rules = "datalog/rules.dl"
export = ["backlinks", "contexts", "related", "references"]
```

After the fixed-point, the compiler resolves each exported relation to a list
of `{ identifier, title, meta }` objects and passes them to the backmatter
template. Only exported relations are resolved — computing node objects for all
relations would be wasteful.

The backmatter template iterates by relation name rather than hardcoded section
names. The user controls which sections appear and in what order.

---

## Watch loop invalidation

The second invalidation axis from Stage 5 survives unchanged in structure. The
only difference is the cache type:

```rust
// Stage 5
pub struct BackmatterCache {
    pub backlinks: BTreeSet<NodeId>,
    pub contexts: BTreeSet<NodeId>,
    pub related: BTreeSet<NodeId>,
    pub references: BTreeSet<NodeId>,
}

// Stage 6
pub type BackmatterCache = HashMap<String, BTreeSet<NodeId>>;
```

The diff logic iterates keys rather than fields, but the logic is the same:
collect nodes whose cache changed into `backmatter_dirty`, add to the render
set, reuse `rendered_body` where only backmatter changed.

The early exit condition gains one more clause: if no `node_meta` facts changed
(metadata maps are identical across all nodes), skip the datalog pass.

---

## Stratification and negation

The `related` query requires negation-as-failure:

```datalog
related(X, Y) :- links_to(X, Y), not is_reference(Y).
```

This is valid in stratified Datalog: `is_reference` has no recursive
dependency on `related`, so the program is stratifiable. The evaluator
computes `is_reference` in an earlier stratum before evaluating `related`.

Cycles through negation are a hard error — the evaluator detects them during
stratification and emits a diagnostic. The user must restructure their rules.

---

## What this does not include

- User-defined relations in Typst source (`#query(...)`, `#datalog(...)` —
  forester-style in-source querying). This would require changes to the Typst
  template and the extraction pipeline and is a separate concern.
- Equality saturation or e-graph features. See `egglog.md` for why egglog
  was not adopted — the one scenario where equality saturation would earn its
  keep (merging equivalent person/contributor nodes across identity) is not
  a current requirement.
- Contextual numbering (Theorem 1.3, Definition 2.4 style). Context-sensitive
  and complex; deferred indefinitely.
