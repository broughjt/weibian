# OCaml Datalog Library Analysis

The OCaml `datalog` library (`~/scratch/datalog`) is the engine forester uses
for its backmatter queries. This document records what it does, how forester
uses it, and what the right relationship to it is for Stage 6.

---

## What the library contains

Two independent implementations:

**Bottom-up** (`src/bottom_up/bottomUp.ml`, ~1,500 lines): Queue-based unit
resolution with incremental fixpoint. When a new fact is added, the index
fires any rules whose first body literal matches. When a new rule is added, it
resolves against existing matching facts. Iterates to a fixed point.

**Top-down** (`src/top_down/Datalog_top_down.ml`, ~2,000 lines): SLG
tabled resolution — a proof forest with memoization. Supports nested/compound
terms, aggregation, and well-founded semantics for non-stratified negation.

Total library: ~5,600 lines. The CLI tools, caml interface layer, and unix
helpers account for another ~1,000 lines.

---

## What forester actually uses

Bottom-up only. Forester wraps it in `Datalog_engine.ml` with a simplified
API over hashconsed vertex symbols, then drives it from `Datalog_eval.ml`:

```ocaml
let run_query (db : D.db) (query : (string, Vertex.t) Dx.query) : Vertex_set.t =
  let answers = D.ask db ~neg:(eval_premises query.negatives)
                      [|D.var_of_string query.var|]
                      (eval_premises query.positives) in
  Vertex_set.of_list @@ List.map (fun a -> a.(0)) @@ D.list_of_answers answers
```

The `~neg` parameter is how forester gets negation: it passes a list of
negative literals to `Query.ask`, which performs anti-join against the current
database. Negation lives in queries, not in rule bodies.

The top-down implementation, aggregates, goal handlers, explanation tracking,
and user-defined rewrite functions are all unused by forester.

---

## The bottom-up core algorithm

At its heart: a discrimination tree index, unit resolution, and a relational
query layer.

**Discrimination tree index**: A trie-like structure that collapses all
variables to a single sentinel `Var(0)`, enabling fast retrieval of
generalizations (rules whose head matches a new fact) and specializations
(facts that match a rule's first body literal). The non-perfect collapse trades
some precision for index compactness.

**Unit resolution loop**: When a fact F arrives, retrieve all clauses whose
first body literal unifies with F. For each such clause, substitute F's
bindings, drop the first body literal, and if the result is ground add it as a
new fact; otherwise re-queue as a shorter clause. Continue until no new facts
are produced.

**Relational query layer**: `Query.ask` builds a lazy relational algebra plan
— `Match`, `Join`, `ProjectJoin`, `AntiJoin` — over the current database and
evaluates it. The `~neg` parameter generates `AntiJoin` nodes against the
negative literals. Results are sets of term arrays.

**Semi-naive evaluation**: The library tracks which facts are "new" each
iteration and only resolves new facts against existing rules, and existing facts
against new rules — avoiding redundant recomputation of already-derived facts.
This matters for recursive rules like transitive closure.

---

## What we need vs. what the library provides

Our use case is flat relations, ground facts, and simple rule bodies:

```
transcludes(X, Y)
links_to(X, Y)
node_meta(X, "key", "value")
```

No nested terms. No compound structures. Constants are either `NodeId(u32)` or
interned strings. Rule bodies are conjunctions with optional negation.

Roughly half the bottom-up implementation handles things we don't need:
goal-based backward chaining, fact and goal subscription handlers, explanation
tracking, user-defined symbol rewrite functions, the `db_goal` mechanism. Strip
those and the algorithm itself is ~600–800 lines of OCaml.

---

## Porting verdict

**Don't do a literal port. Use it as a reference implementation.**

The OCaml uses idioms that fight Rust's ownership model: offset-based
linked-list substitution chains, persistent functional index updates, and
pervasive use of mutable hashtables with OCaml's GC doing the lifetime work.
A direct translation would produce awkward Rust and probably not compile
cleanly.

The right approach: read the OCaml to understand the algorithm and data
structure design, then write a focused Rust implementation targeting our
specific use case. The OCaml de-risks the algorithmic decisions — it's a
working, shipped implementation — without dictating the Rust structure.

**Estimated scope for a focused Rust implementation:**
- Core term/literal/clause types: ~100 lines
- Discrimination tree index: ~200 lines
- Fixpoint loop with semi-naive evaluation: ~150 lines
- Stratified negation (computing strata, evaluating in order): ~150 lines
- Rule text parser: ~200 lines
- Total: ~800 lines

This is more than the earlier 300–400 line estimate, which underestimated the
index and semi-naive machinery. It's far less than a full port would be.

---

## Relationship to Stage 6

The OCaml library answers the question "does this approach work at forester's
scale?" — yes, forester ships it and it handles the full forest. The ground
facts and relations in our use case are no larger and the rule sets are simpler
(no user-defined relations in source, no `\datalog{...}` blocks).

When implementing Stage 6, `~/scratch/datalog/src/bottom_up/bottomUp.ml` is
the primary reference. The discrimination tree implementation (lines ~200–500)
and the unit resolution loop (lines ~700–900) are the two sections most worth
studying before writing the Rust equivalent. The query layer is secondary —
our "queries" are just relation lookups after fixpoint, not the full relational
algebra the OCaml builds.
