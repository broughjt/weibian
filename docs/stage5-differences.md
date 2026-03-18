# Stage 5 — Remaining Differences: v2 vs v1

This document catalogs every structural difference between the current v2 output
(`~/scratch/BlogSource/dist`) and the v1 reference output
(`~/scratch/BlogSource-original/dist`), excluding backmatter (which is Stage 5
work-in-progress). Differences are grouped by layer.

Verification: all findings are grounded in live file reads of `0001.html` /
`0001/index.html` plus source examination of both Rust codebases, Typst
templates, and HTML templates.

---

## 1. Typst template layer

### 1.1 Template structure

| v1 (`BlogSource-original/typ/_template/`) | v2 (`BlogSource/typ/_template/`) |
|---|---|
| `template.typ`, `lib.typ`, `site.typ`, `template-paged.typ`, `bibliography-template.typ`, `poem-template.typ` | `template.typ`, `bibliography-template.typ`, `poem-template.typ` |

v1 emits a **full HTML document** from Typst (`html.html({ html.head({...}) html.body({...}) })`).
The backend extracts `<head>` children and `<body>` children separately.

v2 emits **custom elements** (`wb-node`, `wb-subnode`, `wb-transclude`) that the
backend parses and splits into node entries. No HTML document structure is
emitted from Typst.

### 1.2 Metadata emission

v1: metadata emitted as `<meta>` tags inside `<head>` from Typst:

```html
<meta name="identifier" content="0001">
<meta content="Post" name="taxon">
<meta name="lang" content="en">
<meta name="toc" content="true">
<meta name="export-pdf" content="false">
<title>Gödel's β Function</title>
```

v2: metadata stored in Typst's introspector via `#metadata(...)` in the
`template` shim; no `<meta>` tags emitted. Backend reads metadata from the
compiled document's introspector rather than from HTML `<meta>` elements.

### 1.3 Metadata data model

v1 (`backend.rs`): `metadata: HashMap<String, String>` — single string per key.
Access in templates: `{{ note.metadata.lang }}`, `{{ note.metadata.taxon }}`.

v2 (`compiler.rs`): `metadata: HashMap<String, Vec<String>>` — list of strings
per key. Access in templates: `{{ node.metadata.lang[0] }}`, `{{ node.metadata.taxon[0] }}`.

### 1.4 Link emission from Typst

v1 `ln-html`:
```typst
html.span(class: "link local", html.elem("wb-internal-link", attrs: (target: dest), body))
```
Emits a `<span class="link local">` wrapper around a `wb-internal-link` custom
element.

v2 `ln`:
```typst
html.elem("a", attrs: (href: "wb:" + identifier, "data-counter": str(n)), content)
```
Emits a plain `<a href="wb:...">` anchor with a counter attribute; no span
wrapper.

### 1.5 Citation emission from Typst

v1: `ct-html` emits `<span class="link local"><wb-cite target="...">...</wb-cite></span>`.
Backend renders citations using the separate `citation.html` template
(`<a class="citation">`).

v2: `ct` is an alias for `ln` — citations are emitted identically to internal
links and rendered through the same `link.html` template (`<a class="link
local">`). No distinction between citations and internal links.

---

## 2. Rust backend layer

### 2.1 Site configuration in context

v1: passes a `site` context object (`root_dir`, `trailing_slash`, `domain`) to
every template render call. Templates use `{{ site.root_dir }}` for CSS href,
nav href, and slug href generation.

v2: no `site` context passed to templates. All URLs in templates are hardcoded
or computed in Rust (`format!("/{identifier}.html")`). `weibian.toml` has no
`[site]` section at this point.

### 2.2 URL / href generation

v1 (`build_note_href`):
```rust
if site.trailing_slash { format!("{}{note_id}/", site.root_dir) }
else                    { format!("{}{note_id}.html", site.root_dir) }
```
BlogSource-original uses `trailing_slash = true`, so all links are `/0001/`.

v2:
```rust
let href = format!("/{identifier}.html");
```
Always `.html` extension, always root-relative `/`. `trailing_slash` and
`root_dir` are not consulted.

### 2.3 Output file paths

v1: `trailing_slash = true` → `dist/0001/index.html`.

v2: always `dist/0001.html`.

### 2.4 `<head>` injection vs. static template

v1: Typst emits a full `<head>` block; `note.head` is injected into the outer
`note.html` via `{{ note.head | safe }}`. The `<head>` contains identifier meta,
taxon meta, lang meta, toc meta, export-pdf meta, and `<title>`.

v2: no `note.head` — `node.html` only contains a static `<title>{{ node.title_text }}</title>`.
The identifier, taxon, lang, toc, export-pdf meta tags are not emitted.

---

## 3. `node.html` / `note.html` template differences

Below is a side-by-side of the two page-level template structures.

### 3.1 `<html>` lang attribute

v1:
```html
<html lang="{{ note.metadata.lang }}">
```

v2:
```html
<html lang="en">
```
Hardcoded; does not use `node.metadata.lang`.

### 3.2 CSS stylesheet href

v1 (via Tera using `site.root_dir = "/"`):
```html
<link rel="stylesheet" href="&#x2F;css/weibian.css">
```
The slash is HTML-encoded (`&#x2F;`) because Tera entity-escapes URLs by
default.

v2:
```html
<link rel="stylesheet" href="/css/weibian.css">
```
Literal slash. The CSS href in v2's template should use `{{ site.root_dir }}` once site config is threaded through.

### 3.3 Nav home link

v1: `href="{{ site.root_dir }}"` → `href="&#x2F;"` (entity-encoded).

v2: `href="/"` (hardcoded).

### 3.4 `data-taxon` attribute on `<section>`

v1: `<section class="block" lang="en">` — no `data-taxon`.

v2: `<section class="block"\n  lang="en"\n  data-taxon="Post">` — extra
`data-taxon` attribute.

### 3.5 `<h1>` id attribute

v1: the Typst `_summary_header` emits `<h1 id="0001">` (identifier as id).

v2: `<h1>` with no `id` attribute.

### 3.6 Slug link href

v1: `<a href="/0001/" class="slug">[0001]</a>` (trailing slash).

v2: `<a class="slug" href="/0001.html">[0001]</a>` (`.html` extension).

### 3.7 Date formatting and element

v1 (formatted in Typst `_default-metadata`):
```html
<li class="meta-item">September 22, 2023</li>
```
Date formatted as long month name, no `<time>` element. `<li>` has class
`meta-item`.

v2 (raw ISO 8601 date passed through):
```html
<li><time>2023-09-22</time></li>
```
ISO 8601 format, wrapped in `<time>`. `<li>` has no class.

### 3.8 `<details>` content wrapper

v1: `{{ note.content | safe }}` inside `<details open>` begins with a `<html>`
element (Typst serialization artifact), then `<summary>...</summary>` and body
content. Specifically:

```html
<details open>
  <html>
  <summary><header><h1 id="0001">...</h1>...</header></summary>
  <p>...</p>
  ...
  </html>
</details>
```

v2: clean separate elements:
```html
<details open>
  <summary>
    <header>
      <h1>...</h1>
      ...
    </header>
  </summary>
  [body content]
</details>
```

The `<html>` wrapper in v1 is a rendering artifact from how the v1 Typst template
emits `html.html(...)` and how the v1 backend's scraper serialises the extracted
body children. v2's approach — custom `wb-node` elements — avoids this entirely.

### 3.9 Authors element

v1: `<address class="author">` (class present).

v2: `<address>` (no class).

### 3.10 Table of Contents

v1: full TOC sidebar rendered from `note.toc` (a nested heading tree), guarded
by `{% if heading_count != 0 %} {% if note.metadata.toc != "false" %}`.

v2: no TOC. Neither the `node.html` template nor the compiler currently
generate a TOC.

### 3.11 Backmatter footer

v1: `<footer>` with backmatter sections (Contexts, References, Backlinks,
Related), each as `<section class="block hide-metadata">` rendered from
transclusions.

v2: no footer, no backmatter (Stage 5 in-progress).

---

## 4. `transclusion.html` template differences

### 4.1 Section class

v1:
```html
<section class="block{%- if not transclusion.show_metadata %} hide-metadata{%- endif -%}"
         lang="{{transclusion.metadata.lang}}">
```
Adds `hide-metadata` class when `show_metadata` is false.

v2:
```html
<section class="block"
  lang="..."
  data-taxon="...">
```
Never adds `hide-metadata`. Always adds `data-taxon` when present.

### 4.2 Summary / title structure

v1 (when `show_metadata = true`):
```html
<h1>{{ transclusion.metadata.title }}</h1>
<details [open]>
  content
</details>
```
No `<summary>`, no taxon span, no slug link. Just a bare `<h1>` above `<details>`.

v2:
```html
<details [open]>
  <summary>
    <header>
      <h{{ hl }}>
        [taxon span]
        [title]
        <a class="slug" href="/identifier.html">[identifier]</a>
      </h{{ hl }}>
      [metadata div with date/authors]
    </header>
  </summary>
  [body]
</details>
```
Always renders a full `<summary><header>` structure with taxon, title, and slug
link. Heading level demoted by `demote-headings`.

### 4.3 Template context shape

v1 context:
```
transclusion.target          (str)
transclusion.content         (str — rendered body HTML)
transclusion.show_metadata   (bool)
transclusion.expanded        (bool)
transclusion.disable_numbering (bool)
transclusion.demote_headings (usize)
transclusion.metadata        (HashMap<String, String>)
```

v2 context:
```
transclusion.identifier              (str)
transclusion.resolved                (bool)
transclusion.title                   (str — HTML)
transclusion.title_text              (str — plain text)
transclusion.body                    (str — rendered body HTML)
transclusion.metadata                (HashMap<String, Vec<String>>)
transclusion.transclusion_metadata   (HashMap<String, Vec<String>>)
```
`show_metadata`, `expanded`, `disable_numbering`, `demote_headings` are now
read from `transclusion.transclusion_metadata["show-metadata"][0]` etc. rather
than as typed boolean/usize fields.

### 4.4 Unresolved transclusion handling

v1: errors at compile time — dangling transclusions abort rendering.

v2: renders `<wb-missing identifier="...">` placeholder for unresolved
transclusions; a process-time warning is emitted but rendering continues.

---

## 5. `link.html` / `internal_link.html` template differences

### 5.1 Link class and element structure

v1 `internal_link.html`:
```html
<a href="{{ link.href }}">{{ link.text | safe }}</a>
```
No class on the `<a>`. The `class="link local"` comes from the Typst `ln-html`
span wrapper: `<span class="link local"><a>...</a></span>`.

v2 `link.html`:
```html
<a class="link local" href="{{ link.href }}">...</a>
```
Class on the `<a>` directly. No span wrapper.

### 5.2 Template context shape

v1:
```
link.target   (str)
link.text     (str)
link.href     (str)
```

v2:
```
link.identifier       (str)
link.href             (str)
link.content          (str — inner HTML of <a> element)
link.resolved         (bool)
link.title            (str — HTML title of target, if resolved)
link.title_text       (str — plain text title of target)
link.metadata         (HashMap<String, Vec<String>>, if resolved)
link.link_metadata    (HashMap<String, Vec<String>>)
```

### 5.3 Citation distinction

v1: citations use a separate `citation.html` template → `<a class="citation">`.

v2: citations (emitted by `ct`) use the same `link.html` as internal links →
`<a class="link local">`.

---

## 6. Config differences

| Setting | v1 (`.wb/config.toml`) | v2 (`weibian.toml`) |
|---|---|---|
| Config location | `.wb/config.toml` | `weibian.toml` (project root, found by upward search) |
| Template discovery | All files under `.wb/templates/*.html` auto-loaded by Tera | Explicit paths: `node_template`, `transclusion_template`, `link_template` |
| Site domain | `[site] domain = "..."` | Not present |
| Site root_dir | `[site] root_dir = "/"` | Not present |
| Trailing slash | `[site] trailing_slash = true` | Not present |
| Input dir | `[files] input_dir = "typ"` | `input_directory = "typ"` |
| Output dir | `[files] output_dir = "dist"` | `output_directory` (defaults to `dist`) |
| Public dir | `[files] public_dir = "..."` | `public_directory = "..."` |

---

## 7. Summary: what v2 is missing (non-backmatter)

| Feature | v1 | v2 status |
|---|---|---|
| `<html lang>` from metadata | `lang="{{ note.metadata.lang }}"` | Hardcoded `lang="en"` |
| `<head>` meta tags (identifier, taxon, lang, toc, export-pdf) | Injected via `{{ note.head \| safe }}` | Not emitted |
| CSS/nav/slug hrefs via `site.root_dir` | `{{ site.root_dir }}` | Hardcoded `/` |
| Trailing-slash URL support | `trailing_slash = true` → `/0001/` | Always `.html` |
| `<h1 id="identifier">` | Yes (from Typst `_summary_header`) | Not present |
| Date formatted as long form | "September 22, 2023" | ISO 8601 "2023-09-22" in `<time>` |
| `<li class="meta-item">` | Yes | Plain `<li>` |
| `<address class="author">` | Yes | Plain `<address>` |
| Table of Contents | Full TOC sidebar | Not implemented |
| `hide-metadata` class on transclusions | Yes, when `show_metadata=false` | Not present |
| Transclusion title: bare `<h1>` without slug | Yes | Always full `<summary><header>` with slug |
| Citation `<a class="citation">` | Yes | Same as internal link |
| No `data-taxon` on section | Correct (v1 has none) | v2 adds `data-taxon` |
| `<html>` wrapper inside `<details>` | Present (Typst artifact) | Not present (v2 is cleaner) |

Items where **v2 is cleaner than v1** (no need to replicate):
- No `<html>` wrapper inside `<details>` (v1 artifact)
- Unresolved transclusions produce a warning + placeholder rather than hard
  error
