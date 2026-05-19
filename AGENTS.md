# AGENTS.md

This file provides guidance to coding agents when working with code in this repository.

## What This Is

Weibian (`wb`) is a static site generator for scientific note-taking built on top of Typst. It compiles `.typ` files to HTML with transclusion, internal link resolution, and backmatter generation (backlinks, contexts). The core challenge is an incremental compiler with a file dependency graph that enables efficient watch-mode rebuilds.

## Commands

```bash
# Build
cargo build --release

# Run
wb build          # single pass
wb watch          # incremental watch mode (also: wb w)

# Lint / format
cargo clippy --all-targets -- --deny warnings
cargo fmt

# Test (infrastructure exists but tests are currently commented out — runs 0 tests)
cargo test
cargo test <test_name> -- --nocapture

# Nix dev shell (includes rust-analyzer, cargo-edit, cargo-machete)
nix develop
```

## Architecture

Weibian runs a **two-stage pipeline** per `.typ` file, then global post-processing:

### Per-file stages

1. **Extract** (`src/compiler/extract.rs`) — invokes `typst-kit` to compile a `.typ` file to HTML, then parses the DOM to find node declarations (`#show: node(...)`, `#subnode(...)`), transclusion markers, internal link references, and metadata. This is the largest and most complex module (~1900 LOC).
2. **Process** (`src/compiler.rs`, `src/compiler/render.rs`) — integrates extracted nodes into the compiler's incremental state, resolves transclusions and links across all files, computes backmatter (backlinks, contexts, related notes), and applies Jinja2 templates (via `minijinja`) to produce final HTML.

### Global post-processing

After all files are extracted, `_process` runs once:
- **Transclusion resolution** — topological sort across all nodes to determine embedding order.
- **Backmatter** — compute backlinks, contexts, related notes across the full graph.
- **Link resolution** — map internal references to output URLs.
- **Render** (`src/compiler/render.rs`) — apply Jinja2 templates to produce final HTML.
- **Output** — write to `dist/`.

### Incremental watch loop (`src/watch.rs`)

The compiler (`src/compiler.rs`) maintains state across iterations. On a file change it uses the import dependency graph (modeled as a `petgraph` digraph) to reverse-BFS and identify all transitively affected files, then rebuilds only those. `src/file_store.rs` handles selective cache invalidation per `FileId`.

### Key modules

| Module | Role |
|--------|------|
| `src/main.rs` | CLI dispatch via `clap` |
| `src/config.rs` | Config loading (`weibian.toml` via `figment`) + CLI args |
| `src/build.rs` | Typst compilation, FileStore setup |
| `src/compiler.rs` | Incremental compiler state machine |
| `src/compiler/extract.rs` | HTML parsing, node/metadata extraction (~1900 LOC) |
| `src/compiler/render.rs` | Jinja2 template rendering |
| `src/world.rs` | Typst `World` implementation (fonts, stdlib) |
| `src/watch.rs` | File watching (via `notify-debouncer-full`), watch loop |

### Input/output layout

- **Input:** `typ/` directory — `.typ` files declaring nodes via `#show: node(identifier: "...", title: [...], ...)`
- **Output:** `dist/` — per-node HTML files and copied `public/` assets
- **Config:** `weibian.toml` (optional) — include/exclude globs, paths, render settings

## Testing

The test suite uses `proptest` + `proptest-state-machine` to verify that incremental builds match stateless builds. Tests live in `src/compiler/tests.rs` and its submodules. Property tests check invariants like `incremental_matches_stateless`, correct output plan deletes/writes, and disjointness of writes and deletes. There are also targeted unit tests for specific duplicate-identifier edge cases. Proptest regression artifacts live in `proptest-regressions/`.

## Key Dependencies

- `typst`, `typst-html`, `typst-kit`, `typst-syntax` — Typst compiler (pinned to a specific git rev)
- `dom_query` — HTML DOM traversal for extraction
- `minijinja` — Jinja2 templating
- `petgraph` — dependency/import graphs
- `figment` + `serde` — layered config
- `clap` (derive) — CLI
- `globset` — include/exclude patterns
- `comemo` — caching/memoization
- `jiff` — date/time
