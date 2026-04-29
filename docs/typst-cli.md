# Phelps vs. typst-cli: What weibian should take from each

## Core architectural difference

The most important difference is that **typst-cli compiles one file**; its entire incremental
strategy is:

```
watch all dependencies → any changes? → world.reset() → recompile from scratch
```

There's no import graph because there's no need to decide *which* of N sources to recompile.
**Phelps's graph-based approach is the right one for weibian**, where you have N source files and
need BFS to find which ones are affected by a change.

---

## 1. `comemo::evict(10)` — phelps is missing this, weibian must add it

This is the most important thing to take from typst-cli. Typst uses `comemo` for constraint-based
memoization of internal compilation steps. These memos accumulate across compilations. typst-cli
calls `comemo::evict(10)` after every watch cycle — this evicts entries not accessed in the last 10
compilations.

Phelps never calls this. In a long watch session with many compilations this will eventually cause
memory issues.

**For weibian:** call `comemo::evict(10)` at the end of each watch loop iteration, after all
recompilations are done.

```rust
// in watch.rs, inside the for event_result in &rx loop:
let outcomes = rebuild_sources(&mut state, &affected);
report(Trigger::Change(&events), &outcomes);
comemo::evict(10);
```

---

## 2. `world.reset()` semantics

typst-cli calls `world.reset()` which does a full reset of the `FileStore` (clears the dependency
tracking set and all cached file slots). It can do this cheaply because it only ever compiles one
source.

Phelps's selective `slot.reset()` via BFS is the correct approach for multi-file builds — you only
reset slots on the path from the changed file to affected sources, so unchanged files don't need to
re-read and re-hash their content.

The key point is that `comemo::evict()` is the *companion* to world-level reset. typst-cli does
both. Phelps does the slot-level caching but skips the comemo side. **You need both.**

---

## 3. `now` / timestamp handling

typst-cli stores `now` in the world as a `OnceLock<DateTime<Utc>>` (initialized on first call to
`today()`) and clears it in `world.reset()` between compilations. This means:
- All files in one compilation see the same timestamp
- Each new compilation (watch cycle) gets a fresh timestamp

Phelps hard-codes `time: UtcDateTime` in the `State` struct at world-creation time, which is
effectively the same behavior but doesn't support `SOURCE_DATE_EPOCH` for reproducible builds.

**For weibian:** follow phelps's simpler approach for now — capture `OffsetDateTime::now_utc()` at
`SystemWorld::new()` time. `SOURCE_DATE_EPOCH` support can be added later if needed.

---

## 4. Status display — adopt typst-cli's pattern for `report.rs`

typst-cli's `Status` enum is much better UX than phelps's (which has no terminal output — it feeds
a web UI). The pattern to copy:

```
[HH:MM:SS] compiled successfully in 123ms
[HH:MM:SS] compiled with warnings in 456ms
[HH:MM:SS] compiled with errors
```

Key details from typst-cli worth copying:
- **Clear screen before each watch-cycle status** (`out.clear_screen()`)
- **Timestamp per compilation** (makes it easy to see when something last changed)
- **Distinguish `Success` / `PartialSuccess` / `Error`** — warnings get their own status, not
  silently ignored

For the compile (non-watch) path, skip the clear-screen and just print the outcome directly.

---

## 5. Dependency tracking — stick with phelps's explicit graph

typst-cli's `world.dependencies()` yields all accessed `FileId`s as a flat set — this is what the
watcher monitors. There's no concept of which source "owns" which dependency.

For weibian you need more: when `library/template.typ` changes, you need to know *which* of your
500 source files import it (directly or transitively). The `DiGraphMap<FileId, ()>` approach in
phelps is the right data structure for this.

One thing to confirm: phelps uses `Direction::Incoming` for BFS (since edges point
`dependent → dependency`, so incoming means "files that depend on me"). Make sure the edge
direction in `graph.rs` is unambiguous — the mismatch between edge direction and traversal
direction is a common footgun here.

---

## 6. Source slot reset in `handle_modify` — a subtle bug in phelps

Phelps's `handle_modify`:

```rust
let mut bfs = Bfs::new(&self.graph, i);
let mut dependents = Vec::new();

{
    dependents.push(i);   // manually push i first

    while let Some(j) = bfs.next(&self.graph) {  // BFS also visits i as first node!
        if self.is_source.contains(&j) {
            dependents.push(j);
        }
        slots.get_mut(&j).unwrap().reset();
    }
}
```

The comment says "BFS starts by traversing i, so we don't need to do that manually" — but then it
*does* push `i` manually via `dependents.push(i)` before the loop. This pushes `i` twice if `i`
is in `is_source`, meaning it will be recompiled twice.

**For weibian:** let BFS handle everything uniformly:

```rust
let mut bfs = Bfs::new(&self.graph, i);
while let Some(j) = bfs.next(&self.graph) {
    slots.get_mut(&j).unwrap().reset();
    if is_source.contains(&j) {
        dependents.push(j);
    }
}
```

No manual pre-push of `i`. BFS visits `i` first, so it still gets reset and still gets recompiled
if it's a source file.

---

## 7. What to ignore from typst-cli

- **Export cache** (`ExportCache` with frame hashing for PNG/SVG) — not relevant, weibian outputs
  HTML
- **Dependency file writing** (Make/JSON/Zero format) — not needed
- **Stdin input** — not needed
- **`--timings` output to Perfetto** — not needed yet
- **World creation retry loop** — for a CLI batch tool the simpler "fail if input missing" is fine
- **SIGPIPE reset** (`sigpipe::reset()`) — worth adding eventually if output is ever piped, not
  urgent

---

## Summary table

| Concern | Phelps | typst-cli | Weibian recommendation |
|---|---|---|---|
| `comemo::evict()` | **Missing** | `evict(10)` per cycle | **Add it** |
| Multi-file graph | DiGraphMap | Not needed (1 source) | Keep phelps's approach |
| Slot caching | SlotCell fingerprinting | FileStore reset | Keep phelps's SlotCell |
| `handle_modify` double-push | Minor bug | N/A | Fix it |
| Status reporting | Via web UI | Clear + timestamp | Adopt typst-cli style |
| Font loading | Eager in Resources | LazyLock | Eager is fine |
| `now` in world | Per-world capture | OnceLock + reset | Per-world capture is fine |
| Async/tokio | Uses it (drop this) | Not used | Sync design |
