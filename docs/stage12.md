# Implementation Plan: Stages 1 and 2

## Why implement them together

Stage 1 (build) and stage 2 (watch) are implemented together because the Typst world
setup should be designed with watch in mind from day 1.

Phelps makes this clear: the three pieces of state that matter for incremental
compilation — fonts/library, file content cache, and the import dependency graph — all
need to survive across watch iterations. The `SystemWorld` passed to Typst is created
fresh per compilation, but it *borrows* from this persistent state. That's the correct
model for Typst compilation regardless of whether you're running once or in a loop.

If you implement a "simple" one-shot world first — fresh resources, no slots, no graph
— you'd rewrite all of it when adding watch. The slot machinery isn't watch-specific
complexity; it's the right design from the start.

Watch and compile are still implemented in sequence (compile first, then watch). The
difference is only that `build.rs` is designed around a persistent `WatchState` struct
from day 1, so the watch loop is genuinely additive rather than requiring restructuring.

---

## What phelps contributes

Phelps (`/home/jackson/repositories/phelps/backend/src/`) is the previous note-taking
system. Its build and watch machinery is well-designed and directly reusable in
simplified form. Here is what it has and what we draw from it.

### Persistent state across watch iterations

`BuildService` in `build_service.rs:57-72` owns three pieces of state that survive
across compilations:

- `resources: Arc<Resources>` — the Typst library AST and font book, initialized once
  at startup and never reloaded.
- `slots: Arc<Mutex<HashMap<FileId, FileSlot>>>` — one `FileSlot` per file ever
  accessed. Each slot caches the file's parsed source and raw bytes, keyed by a
  content fingerprint (u128 hash). Slots are reset when their file or a dependency
  changes, but the slot entries themselves persist.
- `graph: DiGraphMap<FileId, ()>` — directed import dependency graph. An edge `i → j`
  means file `i` imports file `j`. Updated after each compilation using the dependency
  set returned by the world.

### The SlotCell / FileSlot machinery (`system_world.rs:161-288`)

```rust
pub struct FileSlot {
    id: FileId,
    source: SlotCell<Source>,
    file: SlotCell<Bytes>,
}

struct SlotCell<T> {
    data: Option<FileResult<T>>,
    fingerprint: u128,
    accessed: bool,
}
```

`SlotCell::get_or_init` implements content-addressed caching:
1. If the slot was already accessed in this compilation, return the cached value
   immediately.
2. Otherwise, read the file and compute a fingerprint (hash of raw bytes).
3. If the fingerprint is unchanged and cached data exists, reuse it.
4. Otherwise, reparse and cache the new value.

`FileSlot::reset()` clears the `accessed` flag, forcing a fresh fingerprint check on
the next access. It does not evict cached data — that only happens if the fingerprint
changes.

### Dependency tracking in the world (`system_world.rs:100-159`)

`SystemWorld` implements Typst's `World` trait. Every call to `source()` or `file()`
inserts the requested `FileId` into a `dependencies: Arc<Mutex<HashSet<FileId>>>` set.
After compilation, `world.into_dependencies()` returns this set, which is used to
update the import graph.

### Reverse-BFS for incremental rebuild (`build_service.rs:287-350`)

When a file is modified:

```rust
let mut bfs = Bfs::new(&self.graph, i);
while let Some(j) = bfs.next(&self.graph) {
    if self.is_source.contains(&j) {
        dependents.push(j);
    }
    slots.get_mut(&j).unwrap().reset();
}
```

BFS from the modified file `i` traverses dependents (files that import `i`, directly
or transitively) because the graph edges point from dependent to dependency. All
traversed slots are reset. All traversed source files are recompiled.

### The debounced watch loop (`build_service.rs:57-128, 175-247`)

Uses `notify_debouncer_full` with a 500ms debounce timeout. File change events are
forwarded via an mpsc channel to the build loop. The loop processes Create, Modify, and
Remove events, dispatching to the appropriate handler for each.

### What we are dropping from phelps

Phelps wraps everything in tokio async and an actor model (BuildService, NotesService,
HTTP/Editor services). This is appropriate for its architecture (browser frontend,
WebSocket API, editor protocol) but is overkill for a CLI build tool. We drop:

- Tokio async runtime and `spawn_blocking`
- Actor services and mpsc channels between them
- `CancellationToken` / `TaskTracker` (replaced by a simple Ctrl+C signal handler)
- `NotesService` (process stage concern, not build stage)
- HTTP and editor services entirely

What remains from phelps — `Resources`, `FileSlot`, `SlotCell`, `SystemWorld`, the
import graph, and the debouncer loop — maps directly onto a synchronous design. Typst
compilation is blocking regardless; phelps uses `spawn_blocking` only to avoid blocking
the tokio executor.

---

## Module structure

```
src/
├── main.rs      — entry point, dispatch compile/watch
├── config.rs    — existing
├── build.rs     — WatchState, compile_source, do_full_build
├── graph.rs     — ImportGraph (reverse-BFS, slot resetting)
├── report.rs    — terminal output formatting
└── watch.rs     — debouncer loop (added in step 2)
```

---

## Step 1: compile path

### `build.rs`

Define `WatchState`, the central struct that owns all persistent compilation state:

```rust
pub struct WatchState {
    resources: Resources,
    slots: HashMap<FileId, FileSlot>,
    graph: ImportGraph,
}
```

`Resources` holds the Typst library and font book, initialized once from
`BuildConfig`. `FileSlot` and `SlotCell` are ported directly from phelps's
`system_world.rs`, stripped of the `Arc<Mutex<...>>` wrappers (unnecessary without
async).

`compile_source(state: &mut WatchState, root: &Path, path: &Path) -> BuildResult`
creates a fresh `SystemWorld` borrowing from state, calls the Typst compiler, extracts
the dependency set, updates the graph, resets affected slots, and returns the raw HTML
string plus diagnostics.

`do_full_build(state: &mut WatchState, config: &BuildConfig) -> Vec<SourceOutcome>`
iterates `config.iter_typst_sources()` and calls `compile_source` for each.

### `graph.rs`

`ImportGraph` wraps a directed graph (can use `petgraph::graphmap::DiGraphMap<FileId,
()>` as phelps does, or a plain `HashMap<FileId, HashSet<FileId>>`). Provides:

- `update(source: FileId, deps: HashSet<FileId>, slots: &mut HashMap<FileId,
  FileSlot>)` — replaces old edges for `source`, adds new ones, resets slots for all
  nodes that depend on any changed dependency.
- `affected_sources(changed: &[FileId], is_source: &HashSet<FileId>) -> Vec<FileId>`
  — reverse-BFS from each changed file, collecting source files that need recompilation.

### `report.rs`

Self-contained terminal formatting. Takes `&[SourceOutcome]` and a trigger (initial
build vs. change event) and writes the formatted output. Used identically from compile
and watch paths.

### `main.rs` compile path

```rust
Command::Compile => {
    let mut state = WatchState::new(&config)?;
    let outcomes = build::do_full_build(&mut state, &config);
    report::report(Trigger::Initial, &outcomes);
    if outcomes.iter().any(|o| o.is_failure()) {
        std::process::exit(1);
    }
    Ok(())
}
```

At the end of step 1, `wb compile` is fully working: it initializes persistent state,
compiles all source files, writes raw HTML to `dist/`, reports results, and exits
nonzero on failure.

---

## Step 2: watch path

`watch.rs` wraps the step 1 machinery in a debouncer loop. The same `WatchState` is
initialized once, `do_full_build` is called for the initial build, then:

```rust
pub fn watch(config: &BuildConfig) -> Result<(), WatchError> {
    let mut state = WatchState::new(config)?;
    let outcomes = do_full_build(&mut state, config);
    report(Trigger::Initial, &outcomes);

    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = notify_debouncer_full::new_debouncer(
        Duration::from_millis(500),
        None,
        tx,
    )?;
    watcher.watcher().watch(&config.root, RecursiveMode::Recursive)?;

    for event_result in &rx {
        match event_result {
            Ok(events) => {
                let changed = relevant_changed_paths(&events, config);
                if !changed.is_empty() {
                    let affected = state.graph.affected_sources(&changed);
                    // Slots for affected files already reset by graph.update()
                    let outcomes = rebuild_sources(&mut state, &affected);
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

Slot resetting is already handled inside `graph.update()` (called during
`compile_source`), so the watch loop has no additional invalidation logic of its own.
Watch is purely additive: a loop around the same build machinery, with debouncing and
event filtering on top.
