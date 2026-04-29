# Extract Validation Surface

This document catalogs the checks currently performed by the extraction path in
[`src/compiler/extract.rs`](../src/compiler/extract.rs).

The practical goal is to make it clear what a mock compiler or malformed-output
generator would need to exercise. One conclusion up front: a small
`MockFileMode` enum is not a very good long-term fit for this surface area.
Many failures are not coarse "file modes"; they are local inconsistencies
between HTML, spans, node metadata, and per-counter metadata.

## Scope

There are three relevant functions:

- `collect_node_spans`
- `collect_metadata`
- `extract`

`collect_node_spans` and `collect_metadata` run before `extract`, but from a
testing perspective they are part of the same extraction pipeline and should be
considered together.

## Diagnostics vs Invariants

There are two qualitatively different classes of failures in the current code:

- ordinary diagnostics collected into `EcoVec<SourceDiagnostic>`
- internal invariants enforced with `assert!` / `expect`

The first class is directly testable through normal compiler output. The second
class is more awkward: malformed compile output can currently panic rather than
produce a diagnostic.

That distinction matters for the mock design.

## 1. Span Collection Errors

`collect_node_spans` walks the exported HTML tree and records spans for every
`wb-node` and `wb-subnode`.

Function:
- [`collect_node_spans`](../src/compiler/extract.rs)

Current diagnostic:

- duplicate `wb-node` / `wb-subnode` identifier within a single compiled
  document

This is emitted when the same identifier is seen more than once while walking
the HTML tree.

## 2. Metadata Collection Errors

`collect_metadata` queries Typst metadata elements and partitions them into:

- node metadata keyed by identifier
- transclusion metadata keyed by counter
- link metadata keyed by counter

Function:
- [`collect_metadata`](../src/compiler/extract.rs)

Current diagnostics:

- `"wb-metadata" must be a [kind, discriminant] array`
- `"wb-metadata" must be a two-element [kind, discriminant] array`
- `"wb-metadata" node identifier must be a string`
- `metadata for unknown node: ...`
- `duplicate metadata for node: ...`
- `"wb-metadata" transclude counter must be an integer`
- `"wb-metadata" transclude counter out of range: ...`
- `duplicate metadata for transclusion counter: ...`
- `"wb-metadata" link counter must be an integer`
- `"wb-metadata" link counter out of range: ...`
- `duplicate metadata for link counter: ...`
- `unknown "wb-metadata" kind: ...`

This is already broader than a single `MockFileMode` enum wants to be. Most of
these are not "document modes"; they are local metadata-shape failures.

## 3. Top-Level Extract Errors

`extract` coordinates the overall DOM pass:

- checks cross-file duplicate identifiers
- extracts subnodes deepest-first
- rewrites transcluding subnodes into synthetic `wb-transclude` elements
- extracts the primary `wb-node`

Function:
- [`extract`](../src/compiler/extract.rs)

Current diagnostics:

- cross-file duplicate node identifier via `node_exists(...)`
- `wb-subnode has invalid transclude value: ...`
- `wb-subnode is missing the transclude attribute`
- `source file produced no wb-node`
- `source file produced multiple wb-node elements`

The subnode pass has one especially important semantic effect: if a subnode has
`transclude="true"`, extraction synthesizes a new transclusion counter and
inserts node metadata into the transclusion-metadata map under that counter.

That means malformed output involving subnodes can easily interact with the
metadata/counter invariants later in the pipeline.

## 4. Node Content Extraction Errors

`extract_node_content` validates and lowers a single `wb-node` or `wb-subnode`.

Function:
- [`extract_node_content`](../src/compiler/extract.rs)

Current diagnostics:

- `wb-subnode is missing an identifier`
- `wb-node is missing an identifier`
- `wb-subnode's first child must be a wb-title element`
- `wb-node's first child must be a wb-title element`
- `wb-transclude is missing an identifier`
- `wb-transclude has invalid counter: ...`
- `wb-transclude is missing a counter attribute`
- `link has invalid data-counter: ...`
- `link is missing a data-counter attribute`

Notably, missing per-counter metadata is not itself an error. The code just
falls back to an empty metadata map for that link/transclusion.

## 5. Internal Invariant Panics

These are not user-facing diagnostics. They are assumptions the code currently
enforces internally.

Current invariant failures:

- transclusion counter overflow while assigning synthetic counters
- duplicate identifier "slipped past collect_node_spans"
- no span found for a node identifier
- no span found for an extra `wb-node` identifier
- unconsumed node metadata after extraction
- unconsumed transclusion metadata after extraction
- unconsumed link metadata after extraction

These matter for tests because a malformed mock output can currently crash the
extractor instead of producing an error.

This is the main reason a few high-level `MockFileMode` variants are not enough:
many interesting failures are better described as "make these two structures
inconsistent" than as "put the file in mode X."

## What the Current Surface Suggests for Testing

If the goal is broad extractor coverage, a better direction than growing
`MockFileMode` is:

1. Keep a small number of coarse file modes.
2. Lower a well-formed `MockFile` to `CompileOutput`.
3. Apply one or more output corruptions to the resulting `CompileOutput`.

Examples of useful corruptions:

- remove a `wb-node`
- duplicate a `wb-node`
- strip a node identifier
- strip a `wb-title`
- strip a `wb-subnode` transclude attribute
- corrupt a transclusion counter
- corrupt a link `data-counter`
- leave behind unmatched node metadata
- leave behind unmatched transclusion metadata
- leave behind unmatched link metadata
- attach metadata to an unknown node
- duplicate metadata for the same node or counter

This matches the code much more naturally than expanding `MockFileMode`
indefinitely.

## Practical Conclusion

`MockFileMode` is still fine for a few coarse cases like:

- well-formed output
- compile error
- perhaps one or two gross structural failures

But it should not be the main abstraction for extractor validation. The real
validation surface is a combination of:

- HTML structure
- spans
- node metadata
- transclusion metadata
- link metadata

That points toward a malformed-`CompileOutput` mutator layer rather than a large
enum of fixed modes.
