# Plan: Bring BlogSource Output Close to v1

## Context

`process()` now wraps each node body in a MiniJinja template, but BlogSource has no template
yet and `weibian.toml` still points nowhere. This plan creates a proper `node.html` template
that reproduces v1's visual structure, plus two small code changes that are clearly Stage 4
work: copying a public directory and fixing the transclusion title markup.

**v1 structure (confirmed by reading `_template_old/template.typ` and `weibian.css`):**

- v1's Typst `_body()` calls `_summary_header()` at the top of `<body>`, producing
  `<summary><header><h1>title</h1><div class="metadata">…</div></header></summary>` followed
  by content. `render_note_body` extracts these body children as `note.content`.
- The `note.html` Tera template wraps `note.content` in
  `<article><section class="block"><details open>{{ note.content }}</details></section></article>`.
- Because `note.content` starts with `<summary>`, the final DOM is
  `article > section > details > summary > header > h1`, which is exactly what the CSS targets
  for the large page-title style (`font-size-heading-xl`).
- Transclusions in v1 use the same `<details><summary><header>` structure (the transcluded
  note's `body_html` also starts with `<summary>`).
- CSS and fonts come from `typ/public/css/weibian.css`; v1 copied `public_dir` → `dist/`.

**Current new-code state:**

- Output files in `dist/` are bare HTML fragments (no `<html>/<head>/<body>` wrap).
- Transclusion replacement in `compiler.rs` puts `<h1>` directly inside `<details>` without
  a `<summary>` wrapper, so the block can't be collapsed and the title is unstyled.
- `typ/public/css/weibian.css` is never copied to `dist/`, so CSS would 404.

**What stays deferred (Stage 5):**
- `lang` attribute (needs metadata)
- `.taxon` span, `.slug` identifier link, `.metadata` div (date/author)
- Backmatter sections
- TOC sidebar

---

## Critical files

- `~/scratch/BlogSource/node.html` — create
- `~/scratch/BlogSource/weibian.toml` — add `node_template` and `public_directory`
- `src/config.rs` — add `public_directory` to both config structs and `try_load`
- `src/build.rs` — copy public directory after clearing output
- `src/watch.rs` — same on initial build
- `src/compiler.rs` — fix transclusion title markup

---

## Implementation

### Step 1 — `~/scratch/BlogSource/node.html` (create)

```html
<!DOCTYPE html>
<html lang="en">
  <head>
    <meta http-equiv="Content-Type" content="text/html; charset=utf-8">
    <meta name="viewport" content="width=device-width">
    <link rel="stylesheet" href="/css/weibian.css">
    <link rel="preconnect" href="https://fonts.googleapis.com">
    <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin="anonymous">
    <link href="https://fonts.googleapis.com/css2?family=Libertinus+Sans:ital,wght@0,400;0,700;1,400&family=Libertinus+Serif+Display&family=Libertinus+Serif:ital,wght@0,400;0,600;0,700;1,400;1,600;1,700&display=swap" rel="stylesheet">
    <title>{{ node.title | striptags }}</title>
  </head>
  <body>
    <div id="grid-wrapper">
      <header class="header">
        {% if node.id != "index" %}
        <nav class="nav">
          <div class="logo">
            <a href="/" title="Home">« Home</a>
          </div>
        </nav>
        {% endif %}
      </header>
      <article>
        <section class="block">
          <details open>
            <summary>
              <header>
                <h1>{{ node.title | safe }}</h1>
              </header>
            </summary>
            {{ node.body | safe }}
          </details>
        </section>
      </article>
    </div>
  </body>
</html>
```

### Step 2 — `~/scratch/BlogSource/weibian.toml`

Add two lines:
```toml
node_template = "node.html"
public_directory = "typ/public"
```

### Step 3 — `src/config.rs`

**`WeibianConfig`** — add optional public directory:
```rust
pub public_directory: Option<PathBuf>,
```

**`BuildConfig`** — add resolved public directory:
```rust
pub public_directory: Option<PathBuf>,
```

**`BuildConfig::try_load`** — resolve the path (same pattern as `input_directory`):
```rust
let public_directory = config
    .public_directory
    .map(|p| if p.is_absolute() { p } else { root.join(p) });
```
Include `public_directory` in the `Ok(Self { … })` constructor.

### Step 4 — `src/build.rs`

After `fs::create_dir(&self.config.output_directory)?`, copy the public directory if set.
Use `walkdir` (already a dependency) to walk the source, recreating the relative structure
under `output_directory`:

```rust
if let Some(public_dir) = &self.config.public_directory {
    for entry in WalkDir::new(public_dir) {
        let entry = entry?;
        let rel = entry.path().strip_prefix(public_dir)?;
        let dest = self.config.output_directory.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&dest)?;
        } else {
            fs::copy(entry.path(), &dest)?;
        }
    }
}
```

### Step 5 — `src/watch.rs`

Same block, after `fs::create_dir(&self.config.output_directory)?` in `watch()`.
(No need to re-copy on incremental rebuilds — public assets don't change while watching.)

### Step 6 — `src/compiler.rs`

Fix transclusion replacement so the title is in a `<summary><header>` wrapper,
matching the CSS's `details > summary > header > h1` selector and making the block
collapsible:

```rust
// Before:
"<section class=\"block\"><details open><h1>{}</h1>{body}</details></section>"

// After:
"<section class=\"block\"><details open><summary><header><h1>{}</h1></header></summary>{body}</details></section>"
```

---

## What this achieves vs. v1

| Feature | v1 | After this plan |
|---------|-----|-----------------|
| Full HTML document | ✓ | ✓ |
| CSS + Libertinus fonts | ✓ | ✓ |
| "« Home" nav | ✓ | ✓ |
| Page title (`<title>`) | ✓ | ✓ |
| Main content in `details > summary > header > h1` | ✓ | ✓ |
| Transclusions collapsible with title | ✓ | ✓ |
| `.taxon`, `.slug`, `.metadata` | ✓ | ✗ (Stage 5) |
| `lang` attribute | ✓ | ✗ (Stage 5) |
| Backmatter sections | ✓ | ✗ (Stage 5) |
| TOC sidebar | ✓ | ✗ (Stage 5) |

---

## Verification

1. `cargo build` — clean compile.
2. `cd ~/scratch/BlogSource && wb build`
3. Open `dist/0001.html` — should be a full HTML page with the Libertinus fonts, CSS styling,
   "« Home" nav, and the title in a large heading.
4. Open `dist/index.html` — should show the index page without the "« Home" nav.
5. Confirm `dist/css/weibian.css` exists (public directory was copied).
6. Open a page that has transclusions (e.g. `dist/index.html`) — transcluded sections should
   have collapsible `<details>` with the section title in the summary.
