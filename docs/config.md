# Configuration Design

## Principle: config-file-first

Following forester's lead, the config file is the source of truth for everything
project-specific. CLI flags are reserved for things that legitimately vary per
invocation or are machine-specific. Almost everything that is currently a CLI flag
belongs in `.wb/config.toml`.

The key question for each flag: does it change per invocation, per project, or per
machine?

- **Per-project** (same every time you build this project, different between projects)
  → config file.
- **Per-invocation** (legitimately varies at the command line) → CLI flag.
- **Per-machine** (local font directories, package cache location) → env var.

---

## Verdict for each current flag

| Flag | Verdict | Reason |
|---|---|---|
| `--config-file` | CLI only | Bootstrapping — needed to find the config |
| `--root` | CLI only | Project selection |
| `--output` | Config, optional CLI override | Almost always the same per project; keep as CLI override for CI use |
| `--public-dir` | Config only | Never changes per invocation |
| `--site-domain` | Config only | Project-specific |
| `--site-root-dir` | Config only | Project-specific |
| `--trailing-slash` | Config only | Project-specific |
| `--pdf-standard` | Config only | Project-specific |
| `--input key=val` | CLI only | Legitimate runtime injection (e.g. `--input draft=true`) |
| `--font-path` | CLI + maybe config | Machine-specific, but projects can also ship their own fonts |
| `--ignore-system-fonts` | Config | Usually a project-level decision |
| `--package-path` | Drop CLI, keep env var | `TYPST_PACKAGE_PATH` already works |
| `--package-cache-path` | Drop CLI, keep env var | `TYPST_PACKAGE_CACHE_PATH` already works |
| `--creation-timestamp` | Drop CLI, keep env var | `SOURCE_DATE_EPOCH` is the standard |

---

## What's left on the CLI

```
wb [--config-file PATH] [--root DIR] compile [--output DIR] [--input key=val]... [--font-path DIR]...
wb [--config-file PATH] [--root DIR] watch   [--output DIR] [--input key=val]... [--font-path DIR]...
wb [--config-file PATH] [--root DIR] new     [--prefix STR] [--taxon STR]
wb [--root DIR] init
```

`BuildArgs` (shared between compile and watch) shrinks to just `--output`, `--input`,
and `--font-path`. Everything site/output/PDF-related moves into the config file. The
current `SiteArguments` struct disappears from the CLI layer entirely — those fields
stay in `SiteConfig`/`SiteSettings` for the build config, they just have no CLI
counterparts.

---

## Config file structure

```toml
[directories]
source = "typ"      # where .typ files live; default "typ"
output = "dist"     # default "dist"
public = "public"   # default "public"

[build]
pdf_standard = ["a-2b"]   # optional; omit for no conformance enforcement
ignore_system_fonts = false

[site]
domain = "https://example.com"
root = "/"
trailing_slash = false

[files]
include = ["**/*.typ"]
exclude = []
```

Note: `[directories]` should gain a `source` field that does not exist in the current
config. Right now `iter_typst_sources` walks from `self.root`, traversing the entire
project tree. Walking from a configured source directory (defaulting to `"typ"`) is
both faster and less surprising. This should be added to `FilesConfig` early.

---

## Resulting `Arguments` shape

```rust
pub struct Arguments {
    #[arg(long, global = true)]
    pub config_file: Option<PathBuf>,

    #[arg(long, global = true, env = "WEIBIAN_ROOT")]
    pub root: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

pub struct BuildArgs {
    #[arg(long)]
    pub output: Option<PathBuf>,           // CI override only

    #[arg(long = "input", value_name = "key=value")]
    pub inputs: Vec<(String, String)>,

    #[arg(long = "font-path")]
    pub font_paths: Vec<PathBuf>,
}

pub struct NewArgs {
    #[arg(long)]
    pub prefix: Option<String>,

    #[arg(long)]
    pub taxon: Option<String>,
}
```

`BuildConfig::try_load` takes `(root: Option<PathBuf>, config_file: Option<PathBuf>,
args: BuildArgs)` and merges CLI args over config file values. The command is extracted
by the caller before `try_load` is called — config loading and command dispatch are
cleanly separated.
