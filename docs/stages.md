# Incremental Implementation Stages

## Why build and process are decoupled

The entire interface between them is one type:

```rust
pub enum BuildResult {
    Success { html: String, warnings: Vec<Diagnostic> },
    Failure { errors: Vec<Diagnostic> },
}
```

Build is a pure function from `(source_path, world)` to `BuildResult`. Process is a
consumer of `html: String`. Neither needs to know anything about the other's internals.
The only design discipline required: **don't write output from inside build.rs**. Build
returns HTML in memory; whoever calls build decides what to do with it. Maintain that
and you can swap in the real process at any time without touching build.

The one genuine coupling risk: output filenames. In stage 1, `ab1c.typ` naturally maps
to `dist/ab1c.html`. In the final design, process decides the output structure —
potentially `dist/ab1c/index.html` for clean URLs, plus separate pages for subnodes.
This will change. That's fine for an intermediate milestone; just don't let any other
code start depending on the stage-1 output path scheme.

---

## Before starting: two things to fix

**1. Command dispatch in `main.rs`**

`BuildConfig::try_load` currently discards the `Command` variant, so `dispatch` can't
differentiate compile from watch. Extract the command before calling `try_load`:

```rust
fn dispatch(arguments: Arguments) -> Result<(), ...> {
    let command = arguments.command.clone();
    let config = BuildConfig::try_load(arguments)?;
    match command {
        Command::Compile(_) => { ... }
        Command::Watch(_) => { ... }
    }
}
```

**2. Dead code warnings in `config.rs`**

`SiteSettings` and several `BuildConfig` fields are dead because they're process-stage
concerns (URL generation, output structure) that aren't needed yet. Suppress with
`#[allow(dead_code)]` for now. Don't feel obliged to use them to make the warnings go
away.

---

## Stage 1 — Build + compile command

**New files:** `src/build.rs`, `src/report.rs`

- `build.rs`: Typst world setup, compile one source file → `BuildResult`. Track the
  import graph (which files were accessed during compilation) from the start — the watch
  loop needs it and it's part of the compilation machinery, not something to retrofit
  later.
- `report.rs`: Pretty terminal output — formatted diagnostics and the
  `"Done. N files. (0.8s)"` style separator. Worth doing now; it's self-contained and
  used from every subsequent stage.
- `main.rs`: Fix command dispatch. `wb compile` runs a full build over all sources,
  writes raw Typst HTML to `dist/` (one file per source, stem name), exits nonzero on
  any failure.

**Outcome:** A working `wb compile` that produces real Typst-compiled HTML in `dist/`.
Output won't have transclusion, backmatter, or link processing, but it's a real build
you can run against your notes.

---

## Stage 2 — Watch loop

**New files:** `src/graph.rs`, `src/watch.rs`

- `graph.rs`: `ImportGraph` type — maps source file → set of files accessed during its
  compilation. Exposes `affected_sources(changed: &[PathBuf]) -> Vec<PathBuf>` via
  reverse-BFS. (The transclusion graph will also live here in stage 4, but for now only
  the file import graph is needed.)
- `watch.rs`: File watching with `notify-debouncer-full`, calls the stage 1 build
  machinery for affected sources, calls report.

**Outcome:** A working `wb watch` that rebuilds only the affected Typst files on each
change. Still raw HTML output, but incrementally rebuilt.

---

## Stage 3 — Process: node extraction

**New files:** `src/process.rs`

- Parse raw HTML output, extract the file node (and subnodes via `<wb-subnode>`).
- `NodeStore` with `raw_html`, `transclusions`, `metadata` per node.
- Write extracted node HTML to proper output paths.
- No transclusion resolution yet — `<wb-transclusion>` elements are left in place or
  stripped.

This is when the output path scheme changes for real. Clean URL structure
(`dist/ab1c/index.html`) can be introduced here.

**Outcome:** `wb compile` produces per-node output pages with correct URLs. Transclusion
placeholders are present but unresolved.

---

## Stage 4 — Transclusion resolution

- Add the transclusion graph to `graph.rs` (alongside the existing import graph).
- Topological sort in `process.rs`, substitute `<wb-transclusion>` elements with
  rendered content from the NodeStore.
- Incremental invalidation in the watch loop: on a change, re-render the affected
  transclusion subgraph (reverse walk of the transclusion graph) in addition to the
  already-tracked file import subgraph.

**Outcome:** Transclusion works. Nodes render with their transcluded content inline.
Watch loop correctly invalidates nodes whose rendered body depends on changed content.

---

## Stage 5 — Datalog + backmatter

- Add a datalog database to the watch loop state (persistent across watch iterations).
- Extend `NodeStore` entries with `contributed_facts` and `cached_query_results`.
- Backmatter queries (backlinks, contexts, related, references) and rendering.
- Second invalidation axis in the watch loop: after rebuilding the datalog database,
  diff query results against cached results to find nodes whose backmatter changed;
  re-render those alongside the transclusion-invalidated set.

See `forester.md` for the detailed design of the datalog integration and its interaction
with the NodeStore and watch loop.

**Outcome:** Full backmatter generation with transitive reference propagation. Watch loop
correctly handles both invalidation axes.
