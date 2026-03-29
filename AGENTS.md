# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Weibian** is a static site generator for scientific note-taking (inspired by Forester). It compiles **Typst** source files into an HTML website with transclusion, backlinking, and structured content organization.

Notes are organized into **nodes**:
- **File nodes**: Primary node declared with `#show: node(...)` in a `.typ` file
- **Subnodes**: Secondary nodes declared with `#subnode(...)`, zero or more per file

## Commands

```bash
# Development
cargo build                     # Debug build
cargo build --release           # Release build
cargo run -- build              # Compile notes (one-shot)
cargo run -- watch              # Watch mode with incremental recompilation

# Code quality
cargo clippy --all-targets -- --deny warnings
cargo fmt

# Install binary locally (binary name: wb)
cargo install --path .
```

With the binary installed:
```bash
wb build          # or: wb compile, wb b
wb watch          # or: wb w
wb compile --input draft=true   # Pass Typst inputs via CLI
```

**Config file**: `weibian.toml` — auto-discovered by walking up from CWD, or override with `--config-file`.

## Architecture

### Two-Stage Pipeline

Each `.typ` source file goes through two stages:

1. **Build stage** (`build.rs` → `world.rs`): Invokes the Typst compiler, producing raw HTML in memory. Pure function, no side effects.
2. **Process stage** (`compiler.rs`): Consumes all built HTML, resolves transclusions, computes backlinks, and writes final output files.

### Core Modules

- **`compiler.rs`** (largest, ~1175 lines): Central `Compiler` struct holding all node state. Maintains `transclusions` and `links` as `DiGraphMap`s, tracks `file_to_nodes`/`node_to_file` mappings, and dirty/removed sets for incremental builds. The `process()` method runs Tarjan's SCC to detect cycles, topologically sorts nodes for rendering order, and produces an `OutputPlan`.

- **`config.rs`**: CLI argument parsing (`Arguments` via clap), TOML deserialization (`WeibianConfig`), and fully resolved `BuildConfig`. Sets up the minijinja environment with custom filters (`demote_headings`, `disable_numbering`).

- **`build.rs`**: `Builder` struct — discovers `.typ` files via glob, compiles each through Typst, then hands results to `Compiler::process()`.

- **`world.rs`**: Implements the `typst::World` trait. `SystemWorld` is created ephemerally per file compilation; `Resources` holds the font store and Typst stdlib.

- **`file_store.rs`**: Per-file slot cache distinguishing loaded (bytes) vs parsed (source) state. Enables fast re-parsing in watch mode by reusing stale sources when file content hasn't changed.

- **`watch.rs`**: `notify-debouncer-full`-based file watcher (500ms debounce). Tracks the import graph to identify which source files are affected by a change, then selectively invalidates the cache and re-renders only impacted nodes.

### Incremental Compilation

Two invalidation axes work together in watch mode:
1. **Import graph**: Which Typst files `#import` which others — used to find all transitively affected sources when a file changes.
2. **Transclusion graph**: Which nodes transclude which — used to determine rendering order and re-render nodes whose content has changed.

### Templating

Output HTML is rendered via **minijinja** (Jinja2-compatible). Templates are loaded from the config-specified paths for `node.html`, `transclusion.html`, and `link.html`.

### Datafrog (Planned)

`datafrog` (incremental datalog engine) is included as a dependency for future backmatter queries (transitive backlinks, related notes). See `docs/transclusion.md` for the design.

## Documentation

The `docs/` directory contains detailed architecture notes:
- `stages.md` — 6-stage implementation plan and current progress
- `transclusion.md` — transclusion/backmatter deep-dive
- `architecture-thoughts.md` — high-level design decisions
- `config.md` — configuration system design
