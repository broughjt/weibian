# Architecture Thoughts

## Overview

Weibian v2 compiles a collection of Typst source files into a static site. Each
source file may contain one or more *nodes* (the fundamental unit of the site,
borrowed from org-roam's terminology). The pipeline has two distinct stages per
source file: **build** and **process**. These stages sit inside a **watch loop**
during development.

---

## Node Identity and Naming

Identifiers are plain strings — no special prefix convention. The preference (following
forester) is 4-digit base-36 strings (e.g. `"ab1c"`, `"0034"`, `"zz9k"`), but
any string is valid. Exceptions are common for person notes, reference notes, and
article notes (e.g. `"hanwen-guo"`, `"hott-book"`).

There are two kinds of nodes, mirroring org-roam's distinction:

- **File node**: the primary node of a `.typ` file, declared with `#show: node(...)`.
  Its identifier is typically the filename stem (e.g. `ab1c.typ` → node `"ab1c"`).
- **Subnode**: a node that lives inside a file alongside the file node, declared
  with `#subnode("id", [Title], ...)[ body ]`. Zero or more per file.

---

## Pipeline Architecture

```
For each .typ source file:

  [source.typ]
       │
       ▼
  ┌─────────┐
  │  build  │  typst compile source.typ → source.html
  └─────────┘
       │
  BuildResult (Success | Failure)
       │
       ▼ (only on Success)
  ┌───────────┐
  │  process  │  source.html → extract file node + subnodes
  └───────────┘
       │
  ProcessResult (Success | Failure)
```

Then, globally across all nodes:

```
  [all nodes from all process stages]
       │
       ▼
  ┌────────────────────────────┐
  │  transclusion resolution   │  topological sort + render
  └────────────────────────────┘
       │
       ▼
  [output HTML files written to dist/]
```

The two failure modes are categorically different:

- **Build failures** are user errors — bad Typst. The user needs structured
  diagnostics (file, line, message, hints). The watch loop must survive these;
  do not run process for a file whose build failed.
- **Process failures** are internal errors — malformed HTML output, IO failures.
  These may indicate a bug.

---

## The `node` and `subnode` Functions

We borrow the term *node* from org-roam, which defines it as "any headline or
top level file with an ID." A node is simply a unit of content with an
identity. Org-roam makes no distinction between a file and a section within a
file — both are nodes if they have IDs, and both participate equally in the link
graph.

We carry this directly into weibian. The distinction between `node` and `subnode`
is purely about *where* the node lives in the source, not about what kind of
thing it is. A subnode is a full node — it has an identifier, title, metadata,
its own output page, and participates in transclusion and backlinking just like
any other node. The only difference is that it is authored inline within another
file rather than as a file of its own, and it is implicitly transcluded into its
parent's page.

The adaptation from org-roam to weibian is that we make the node declaration
explicit in the Typst source rather than inferring it from org heading structure.
In org-roam, any headline with an `#+ID:` property becomes a node automatically.
In weibian, the author calls `#node(...)` or `#subnode(...)` explicitly. This is
a deliberate choice: Typst doesn't have org-mode's heading/property-drawer
structure, and explicit declaration makes the metadata (title, taxon, tags, date)
much cleaner to express.

### Renaming `template` to `node`

The current `template` function in `typ/_template/template.typ` is renamed to
`node`. It declares what the file *is*. The call site changes from:

```typst
#show: template(identifier: "ab1c", title: [My Note], tags: ("math",))
```

to:

```typst
#show: node(identifier: "ab1c", title: [My Note], tags: ("math",))
```

### `subnode`: Multiple Nodes Per File

Weibian v2 supports multiple nodes per file (needed for literate Agda in barb,
where one `.lagda.typ` file naturally contains several nodes).

`subnode` takes the same metadata arguments as `node`, plus a content body,
following the same pattern as `theorem` and `proof` in ~/repositories/notes:

```typst
#subnode("bar", [A Title], taxon: "Definition", tags: ("math",))[
  Content of the subnode goes here...
]
```

Positional arguments: identifier (string), title (content). Additional keyword
arguments mirror `node`: `taxon`, `tags`, `date`, etc. The body is passed in
trailing brackets.

In HTML mode, `subnode` emits a `<wb-subnode>` element containing both the
metadata (as attributes) and the compiled content (as children):

```html
<wb-subnode id="bar" title="A Title" taxon="Definition">
  <p>Content of the subnode...</p>
</wb-subnode>
```

In paged/PDF mode, `subnode` renders as a self-contained section with no special
wrapper — the metadata just isn't embedded.

### Subnodes Are Implicitly Transcluded in the Parent

The `<wb-subnode>` element serves double duty in the `process` stage:

1. **Split point**: the process stage extracts each `<wb-subnode>` as its own
   node, giving it its own output page.
2. **Implicit transclusion**: in the parent file node's rendered output, each
   `<wb-subnode>` position is replaced with a transclusion block (the same
   `<section class="block"><details>...` rendering that explicit `#tr()`
   calls produce). The author never needs to write a separate `#tr("bar")` for
   something already declared as a subnode.
   
2. **Implicit transclusion**: 

The parent's content is everything in the compiled HTML that is *not* inside a
`<wb-subnode>` element. Subnodes are rendered expanded by default (this can be
made configurable later, following the pattern of `export-pdf` in `node`).

### Why This is Better than Phelps's Approach

- IDs are explicit and author-controlled, not derived from heading text
- The node/subnode split is semantic (explicit declaration), not syntactic
  (any h2 heading)
- Metadata (title, taxon, tags, date, etc.) lives with the declaration, not
  in a separate system
- The template already has a clear pattern for emitting custom HTML elements
  (`wb-transclusion`, `wb-internal-link`, `wb-cite`)
- Consistent with forester trees and subtrees

---

## Error Types

```rust
// In build.rs or error.rs

pub struct Diagnostic {
    pub severity: Severity,
    pub location: Option<DiagnosticLocation>,
    pub message: String,
    pub hints: Vec<String>,
}

pub struct DiagnosticLocation {
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
}

pub enum Severity { Error, Warning }

pub enum BuildResult {
    Success { html: String, warnings: Vec<Diagnostic> },
    Failure { errors: Vec<Diagnostic> },
}

// In process.rs or error.rs

pub enum ProcessError {
    Parse(String),    // malformed HTML output from typst
    Io(io::Error),    // couldn't write output files
    // add more as needed
}
```

Avoid `StrResult<T>` (i.e. `Result<T, EcoString>`) for the pipeline itself —
it's fine for config loading where errors are one-off strings, but losing
structure means you can't format diagnostics properly. Use it at the boundary
only when bridging to Typst's API.

---

## File-Level Dependency Tracking (Watch Loop)

Each typst file compiles independently. However, source files import shared
library files (e.g. `library/node.typ`, `library/theorems.typ`). Changing a
library file must trigger recompilation of every source file that imports it.

This is the same problem phelps solves with its dependency graph + BFS
traversal. The mechanism comes for free from Typst: the compiler (via
`SystemWorld`) tracks which files were accessed during compilation of each source
file. After compiling `ab1c.typ`, we record: "ab1c.typ depends on
library/node.typ, library/theorems.typ, ...". This builds the import graph.

On a file change event:
1. Mark the changed file as dirty.
2. Walk the reverse import graph (BFS) to find all source files that
   transitively depend on the changed file.
3. Recompile those source files (build + process).

Notes themselves do not import each other at the Typst level — transclusion is
resolved post-compilation in the process stage. So the file-level import graph
should be relatively shallow (source files → library files, not source → source).

---

## Note-Level Transclusion Ordering (Process Stage)

After all source files have been built and split into nodes, transclusion
resolution requires a global topological sort across all nodes, just like
weibian v1's `topo_sort_transclusions()`.

If node A transcludes node B (`#tr("bar")`), B's rendered HTML must exist
before A can be rendered. The process stage:

1. Collect all nodes from all split results.
2. Build the transclusion graph: edge A → B if A transcludes B.
   (Subnodes implicitly create an edge from their parent file node to themselves.)
3. Detect cycles — fail with a clear error if found.
4. Topological sort.
5. Render nodes in order, substituting `<wb-transclusion>` and `<wb-subnode>`
   elements with the already-rendered HTML of the target node.
6. Compute backmatter (contexts, backlinks, related, references) as in weibian v1.
7. Write output HTML files to `dist/`.

This is two separate dependency graphs for two separate concerns:
- **File import graph** (build stage / watch loop): which `.typ` files to
  recompile when a file changes.
- **Transclusion graph** (process stage): what order to render nodes in.

---

## The Watch Loop

```rust
// watch.rs

pub fn watch(config: &BuildConfig) -> Result<(), WatchError> {
    // Compile all source files and build initial state
    let mut state = do_full_build(config);
    report(Trigger::Initial, &state.outcomes);

    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = notify_debouncer_full::new_debouncer(
        Duration::from_millis(300),
        None,
        tx,
    )?;
    watcher.watcher().watch(&config.root, RecursiveMode::Recursive)?;

    for event_result in &rx {
        match event_result {
            Ok(events) => {
                let changed = relevant_changed_paths(&events, config);
                if !changed.is_empty() {
                    let affected = state.import_graph.affected_sources(&changed);
                    let outcomes = rebuild_sources(config, &mut state, &affected);
                    report(Trigger::Change(&events), &outcomes);
                }
            }
            Err(errors) => {
                for e in errors { eprintln!("watch error: {e}"); }
            }
        }
    }

    Ok(())
}
```

`relevant_changed_paths` filters events to `.typ` and `.lagda.typ` files within
the source paths, excluding the output directory to avoid feedback loops.
`import_graph.affected_sources` does the reverse-BFS to find source files to
recompile. When a source file itself changes, it's always in the affected set;
the graph traversal handles library file changes.

---

## Error Reporting

Separate `report.rs`. The interface:

```rust
pub enum Trigger<'a> {
    Initial,
    Change(&'a [DebounceEvent]),
}

pub fn report(trigger: Trigger<'_>, outcomes: &[SourceOutcome]) { ... }
```

Format on build failure:

```
── Change detected ──────────────────────────── 14:23:05 ──

error: undefined variable `bar`
  --> typ/notes/ab1c.typ:12:5
   |
12 │ let x = bar + 1
   |         ^^^
hint: did you mean `baz`?

Build failed (1 error in ab1c.typ)
```

On success:

```
── Change detected ──────────────────────────── 14:23:06 ──

Done. 47 nodes written. (0.8s)
```

The horizontal rule with timestamp acts as a visual separator between builds in
the terminal. `color-print` (already a dependency) handles the coloring. Process
failures should look different from compile failures — "internal error: ..."
with a suggestion to file a bug.

---

## Module Structure

```
src/
├── main.rs        # Entry, dispatch to compile_once or watch
├── config.rs      # Args + config loading (existing)
├── build.rs       # Stage 1: invoke typst per source file → HTML
├── process.rs     # Stage 2: extract nodes, transclusion resolution,
│                  #          backmatter, write output
├── graph.rs       # File import graph + node transclusion graph
├── watch.rs       # Watch loop
└── report.rs      # Terminal output formatting
```

`main.rs` dispatches based on the command (note: `BuildConfig::try_load`
currently discards the `Command` variant — this needs to be threaded through):

```rust
fn dispatch(arguments: Arguments) -> Result<(), ...> {
    let command = arguments.command.clone();
    let config = BuildConfig::try_load(arguments)?;
    match command {
        Command::Compile(_) => {
            let outcomes = do_full_build(&config);
            report::report(Trigger::Initial, &outcomes);
            if outcomes.iter().any(|o| o.is_failure()) {
                std::process::exit(1);
            }
            Ok(())
        }
        Command::Watch(_) => watch::watch(&config),
    }
}
```

---

## What to Drop from Phelps

| Phelps feature | Verdict |
|---|---|
| Tokio async runtime | Drop — no concurrent I/O needed, Typst is blocking |
| Actor services (Build/Notes/HTTP/Editor) | Drop — overkill for a CLI tool |
| UUID-on-heading node identity | Drop — replaced by `#node(...)` / `#subnode(...)` |
| FileSlot/SlotCell cache machinery | Keep concept, simplify — reset slots on recompile |
| CancellationToken + TaskTracker | Drop — simple Ctrl+C signal handler is enough |
| WebSocket/HTTP live reload | Maybe later, separate concern from build pipeline |
| `NotesService` actor | Drop — process stage runs synchronously |

**Keep from phelps:**
- File import graph + reverse-BFS for affected source detection
- Debounced file watching (notify-debouncer-full, 300–500ms)
- The pattern of tracking which files were accessed during typst compilation

---

## Typst Template Changes

In `typ/_template/template.typ`:

- Rename `template-html` / `template` to `node-html` / `node` throughout.
- Add `subnode-html` and `subnode` following the existing HTML/paged dispatch pattern:

```typst
#let subnode-html(id, title, ..attrs, body) = {
  html.elem(
    "wb-subnode",
    attrs: (
      id: id,
      title: plain-text(title),
      // forward taxon, tags, date, etc. from attrs.named()
      ..attrs.named(),
    ),
    body
  )
}

#let subnode-paged(id, title, ..attrs, body) = {
  // Render as a plain content section; no special wrapper needed for PDF
  body
}

#let subnode = if target == "html" {
  subnode-html
} else {
  subnode-paged
}
```

---

## Decided Questions

- **Typst world and slot reuse**: Use a persistent world with slot resetting
  rather than creating a fresh world per build invocation. When a source file
  needs recompiling, reset the slots for that file (and any affected dependents)
  before recompiling, as phelps does. This avoids re-parsing fonts, packages, and
  library files on every build. The goal is to keep the mechanism but write it
  more simply and readably than phelps's `SlotCell<T>` / `FileSlot` machinery.

- **`wb compile` exit code**: Exit with a nonzero code when any source file fails
  to build. This is standard build tool behaviour and makes `wb compile`
  composable in scripts.

- **Node store and process stage granularity**: Split eagerly (immediately after
  each file's build), but render globally with a persistent `NodeStore` that
  survives across watch iterations. The store maps node ID to `{ raw_html,
  rendered_html, transclusions, metadata }`. On a change, only the affected
  subgraph is re-rendered — the changed nodes plus everything that (transitively)
  transcludes them, determined by a reverse walk of the transclusion graph. The
  invalidation rule mirrors what's already done for the file import graph.

  The store lives entirely in memory. Disk spilling was considered but rejected:
  a local build tool holding the full node store in RAM is practical at realistic
  scales, and the complexity of a spilling cache is not worth it. Concrete
  estimates:

  | Scale | Nodes | Raw HTML | Rendered HTML (est. 2×) | Total |
  |---|---|---|---|---|
  | Current notes project | ~400 | ~4 MB | ~8 MB | ~12 MB |
  | Large personal wiki | 2,000 | ~20 MB | ~40 MB | ~60 MB |
  | Very large site | 10,000 | ~100 MB | ~200 MB | ~300 MB |

  The one subtlety is that `rendered_html` grows with transclusion depth — a node
  that transcludes a deep subtree embeds all of that content. But this is the same
  situation weibian v1 already accepts, and in practice transclusion trees don't
  tend to be deeply nested. If memory pressure ever becomes real at a specific
  scale, the right first lever would be to evict `rendered_html` after writing
  output (since `raw_html` is the source of truth and rendered can be recomputed),
  not to introduce disk spilling.
