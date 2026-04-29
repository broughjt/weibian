# v1 vs v2 Template Differences

This document analyzes differences between the v1 (BlogSource-original) and v2
(BlogSource-new) Typst and Jinja/Tera templates, identifying which differences
represent meaningful deficiencies in the v2 compiler compared to v1.

## Structural Architecture

**V1**: Typst produces a **complete HTML document** (`html.html({ head, body })`).
The compiler extracts `note.head` and `note.content` separately. Tera templates
inject `{{ note.head | safe }}` into `<head>` and `{{ note.content | safe }}` into
`<details>`.

**V2**: Typst produces a **fragment** (`<wb-node>` with `<wb-title>`, `<summary>`,
and body content). The compiler extracts metadata via `#metadata()`,
`node.title_text`, and `node.body`. Jinja templates construct `<head>` themselves
from the extracted metadata.

---

## Meaningful Compiler Deficiencies

### 1. No arbitrary `<head>` injection from Typst (minor)

V1 Typst templates produce a complete `<html>` document. The compiler extracts
`<head>` and `<body>` separately via scraper selectors: `head_html` surfaces in
`NoteTemplateContext` and is injected into the page via `{{ note.head | safe }}`;
`body_html` is used for the main render and for transclusions. Crucially, the
`<head>` content is **never** used during transclusion — only `body_html` is
looked up and inlined. So the v1 head-injection capability only affects the main
page render.

V2 Typst produces a fragment instead of a full document. All `<head>` content is
determined by `node.html` using compiler-extracted metadata (flat string key-value
pairs from `#metadata()`). The practical metadata (identifier, taxon, lang, toc,
export-pdf, title) all flows correctly through this path.

The actual limitation is narrower than it first appears: v2 cannot inject
**arbitrary** head HTML from Typst (e.g., a per-node `<link rel="canonical">` or
`<meta property="og:image">`). But in v1, doing so would also require writing it
into every template's `html.head(...)` call, so it's not a major ergonomic win
either way.

**Impact**: Low. The metadata system covers all current uses. Only matters if a
site needs per-node arbitrary head HTML that can't be expressed as a string value.

#### Would changing the `<wb-node>` contract to include `<wb-head>` and `<wb-body>` be worthwhile?

No. Here is the full accounting.

**What you'd gain**: Typst templates could inject arbitrary per-node `<head>`
content — structured data (JSON-LD), Open Graph tags, canonical links, custom
`<link>` elements — without compiler changes. This is the one thing `#metadata()`'s
flat `HashMap<String, Vec<String>>` cannot represent.

**What you'd not gain**: Everything else on the deficiency list. `disable-numbering`
for transclusions, `wb-id-filename-map-file`, and the template gaps are all
unrelated to the node body/head contract.

**The costs**:

- **HTML5 parser breaks `<head>` inside custom elements.** The v2 compiler uses
  `dom_query` (html5ever under the hood), which hoists `<head>` and `<body>` out of
  any parent element to the document root during parsing. The element names would
  have to be `<wb-head>` and `<wb-body>` — already a departure from v1's clean
  `<html>/<head>/<body>` structure.

- **`#metadata()` cannot be eliminated.** Internal coordination — the
  `"wb-metadata": ("transclude", n)` and `"wb-metadata": ("link", n)` numbering
  scheme that links transclusion/link markers to their options — depends on Typst's
  introspector, not HTML. The `#metadata()` system stays regardless. Templates would
  have to write to *both* systems.

- **All three layers change together.** `extract_node_content()` would need to
  select `wb-head` and `wb-body` children separately; `node.html` would need a new
  `node.head_html` variable to inject; every Typst template would need
  `wb-head`/`wb-body` wrappers. That is a coordinated change across compiler,
  Jinja, and Typst for a low-impact benefit.

- **`node.html` already has full `<head>` control.** Because `node.html` constructs
  the entire page, it can add whatever it wants to `<head>` right now using
  `node.metadata` values. The only gap is per-node content that Typst computed but
  couldn't express as a string value. That gap is real but narrow — and addressable
  by extending the metadata schema with a dedicated key (e.g., `"json-ld": ["{...}"]`)
  if the need arises.

### 2. `disable-numbering` for transclusions is stored but not applied

V1's `transclusion.html` applies a `wb_disable_numbering` filter:

```tera
{%- if transclusion.disable_numbering -%}
  {%- set html = html | wb_disable_numbering -%}
{%- endif -%}
```

V2's `transclusion.html` reads `show-metadata` and `expanded` from
`transclusion_metadata`, but never reads or applies `disable-numbering`. Calling
`#tr("wb:id", disable-numbering: true)` in v2 stores the metadata but has no effect
on output. The `demote_headings` filter demotes heading levels but doesn't suppress
numbering.

**Impact**: Low for BlogSource-new (apparently no nodes use this, hence the
comparison passes), but it is an unimplemented feature.

### 3. `wb-id-filename-map-file` not passed to Typst

V1 passes `wb-id-filename-map-file` as a sys.input — a path to a JSON mapping of
node identifiers → file paths. `template-paged.typ` uses it in `tr-paged()` to
`include()` other nodes directly for PDF transclusion:

```typst
let path = id-names-map.at(identifier)
let c = include(path)
```

V2's `try_load()` adds `wb-domain`, `wb-root-dir`, `wb-trailing-slash`, and
`wb-target` to inputs, but not `wb-id-filename-map-file`. `template-paged.typ` is
identical between v1 and v2, so it will fall back to `bytes("{}")` (empty map) and
any `#tr()` in PDF mode will fail at `id-names-map.at(identifier)`.

**Impact**: High for PDF export. PDF mode is currently untested in v2 but will be
broken when attempted.

---

## Template-Level Gaps

These are design choices in the BlogSource-new template that represent regressions
from v1, not limitations of the v2 compiler itself. They can be fixed purely in
`site.typ`, `template.typ`, or `transclusion.html`.

### 4. `Person` taxon handler missing from `site.typ`

V1's `template.typ` includes a built-in `Person` handler rendering `position`,
`affiliation`, `homepage` (with external link), and `orcid` (with orcid.org link).
V2's `site.typ` has `CV`, `Article`, and `Inproceedings` but no `Person`. Any Person
nodes would fall through to `_default-metadata` and show only date/authors.

### 5. `export-pdf` PDF link only in CV taxon

V1's `_default-metadata` renders a PDF link for **any** node with `export-pdf: true`:

```typst
if attrs.at("export-pdf", default: false) {
  _meta-item(link("/pdf/" + attrs.at("identifier", ...) + ".pdf", "PDF"))
}
```

V2's `_default-metadata` doesn't do this. Only the `"CV"` taxon handler in `site.typ`
renders a PDF link. Non-CV nodes with `export-pdf: true` would silently omit the
PDF link.

### 6. `inline-tree` named subnodes: `#id` anchors vs. full page URLs

V1's `inline-tree-html` passes `inline: true` to `_summary_header`, which generates
`href="#identifier"` (anchor link) for the subnode slug. V2's `subnode()` emits
`<wb-subnode>` and relies on the compiler for subnode hrefs — it always generates
the full page URL (via `_summary_header` in `node()`). A named subnode that lives
inside a parent page will have a slug link pointing to `/{id}/` or `/{id}.html`
rather than `#{id}`.

**Impact**: Functional but subtly different UX — the slug link navigates away from
the current page rather than scrolling to the section.

---

## Summary

| Difference | Category | Impact |
|---|---|---|
| No arbitrary `<head>` injection from Typst | Compiler deficiency | Low |
| `disable-numbering` not applied in transclusions | Compiler/template gap | Low |
| `wb-id-filename-map-file` not passed | Compiler deficiency | High (for PDF) |
| `Person` taxon handler missing | Template gap | Low |
| `export-pdf` link only in CV taxon | Template gap | Low |
| `inline-tree` named subnodes: full URL vs `#id` anchor | Template gap | Low (UX only) |

To recap the full list from the doc:

- **`disable-numbering` filter** — fixed in this session
- **`wb-id-filename-map-file` not passed** — still open; blocks PDF export entirely
- **No arbitrary `<head>` injection from Typst** — still open, but assessed as low impact since all practical metadata flows through `#metadata()` already
- **#4, #5, #6** (Person taxon, export-pdf outside CV, named subnode anchor hrefs) — template gaps, not compiler deficiencies, fixable in Typst/Jinja without touching the compiler

So the two remaining compiler-level gaps are PDF support and head injection, and of those, PDF support is the one that actually blocks functionality.
