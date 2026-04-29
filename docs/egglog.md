# Egglog: Analysis and Verdict

## Why it came up

Egglog (`~/scratch/egglog`) was considered as the datalog engine for backmatter
query computation. The initial evaluation concluded that `datafrog` was
sufficient for the hardcoded rules we needed at the time (transitive closure of
transclusions, references query). We moved on.

The question was revisited when the design shifted to generic user-defined
metadata and rules (see `metadata.md`). Runtime rule parsing became a hard
requirement — the user supplies a `rules.dl` file, and the compiler evaluates
it. This eliminates datafrog, which requires rules expressed as Rust closures
at compile time.

---

## What egglog actually is

Egglog is a hybrid system combining equality saturation (e-graphs) with
datalog-style relational queries. It is the successor to the `egg` Rust
library, designed for program optimization and synthesis tasks.

It is **not** a plain datalog engine. The datalog execution runs on top of a
union-find congruence closure. Every fact lives inside the e-graph; every join
runs through equality-aware infrastructure. These two aspects are inseparable
— you cannot use the relational query layer without the e-graph machinery.

---

## What it offers that datafrog does not

**Runtime rule parsing.** Egglog has a full text format (S-expression based,
`.egg` files) and a `Parser::get_program_from_string()` API. Rules, relations,
and queries can be loaded from a file at runtime. This is exactly what
datafrog cannot do.

The `node_meta(X, key, value)` ternary fact pattern works naturally in
egglog's relation model. User-defined derived relations over metadata would
look like:

```lisp
(relation node-meta (String String String))
(relation is-reference (String))
(rule ((node-meta id "taxon" "Reference")) ((is-reference id)))

(relation backlinks (String String))
(rule ((links-to src dst)) ((backlinks dst src)))
```

---

## Why it was ruled out

**The e-graph machinery is inseparable and unused.** Our use case is pure
relational datalog: small relations, simple joins and filters, basic transitive
closure. We have no program optimization use case. The union-find congruence
closure runs on every operation and provides nothing useful for us.

**Heavy dependency footprint.** Egglog pulls in rayon, crossbeam, dashmap, and
mimalloc — a concurrency and performance stack built for optimizer workloads.
For a notes build tool processing a few hundred nodes on each watch event,
this is machinery that does no useful work and adds significant compile-time
and binary weight.

**No stratified negation.** Egglog supports constraint-based negation (`!=`
guards evaluated during query matching) but not negation-as-failure. The
`related` query — nodes this node links to, excluding references — requires:

```datalog
related(X, Y) :- links_to(X, Y), not is_reference(Y).
```

This cannot be expressed directly. The workaround is an explicit complement
relation, but that adds boilerplate and requires the user to understand why
standard Datalog syntax does not work as written.

**Unfamiliar syntax.** The S-expression format is well-designed but
non-standard. Users familiar with Datalog syntax (`:- `, comma-separated body
atoms) would need to learn a different notation for no benefit in our context.

---

## The right tool

Write a small focused evaluator. Our rule language needs are:

- Joins on small in-memory relations (hundreds of nodes, thousands of edges)
- Recursive rules for transitive closure (semi-naive to avoid redundant work)
- Stratified negation for complement filters
- A parser for standard Datalog syntax

This is approximately 300–400 lines of Rust with no new dependencies, and it
matches our semantics exactly including negation-as-failure.

---

## When egglog would be the right choice

Equality saturation is genuinely useful for one class of problem we could
eventually face: recognising that two differently-identified nodes represent
the same entity and merging their edges. For example, a person node `alice`
and a bibliography entry `alice-phd-thesis` that both have
`author: "Alice Smith"` — if you want contributor edges to propagate across
that equality, plain Datalog cannot express it but egglog handles it
naturally. That is not a current requirement, and is likely further out than
Stage 5.
