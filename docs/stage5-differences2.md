# Stage 5 — Remaining Compiler/Config Differences

Two changes to the v2 compiler and config are needed to achieve structural
parity with v1 HTML output. This document describes what each change does and
which concrete HTML differences it addresses.

---

## 1. `config.rs` — `demote_headings` adds `class="disable-numbering"`

**Location:** `demote_headings_html` in `src/config.rs`, applied via the
`demote_headings` Jinja filter.

**What it does:** After renaming a heading tag (e.g. `h1` → `h2`) during
transclusion, also adds `class="disable-numbering"` to the element.

**What HTML difference it fixes:** In v1, `_summary_header` in `template.typ`
always sets `class="disable-numbering"` on headings that have been demoted. v2
previously renamed the tag but left the class absent.

This affects every transcluded node where `demote-headings` is applied — which
is every `#tr(...)` call site that uses the default `demote-headings: 1`. For
example, a transcluded article whose title heading is demoted from `<h1>` to
`<h2>` would be missing the class without this fix:

```html
<!-- v1 -->
<h2 class="disable-numbering" id="0001">...</h2>

<!-- v2 without fix -->
<h2 id="0001">...</h2>
```

---

## 2. `compiler.rs` — `wb-extra-meta` element extraction

**Location:** `extract_node_content` in `src/compiler.rs`.

**What it does:** Before capturing `raw_html` from a `wb-node`, the compiler
scans for `<wb-extra-meta>` child elements, collects their inner HTML as
strings, removes them from the body, and prepends them to
`node.metadata["extra-meta"]`. The Jinja templates then render these strings
into the `<div class="metadata"><ul>` just like any other `extra-meta` items.

**Why it's needed:** Some metadata items are Typst content objects (links,
`context` blocks) that cannot be serialized through `#metadata(...)` —
`normalize_metadata` would produce useless `repr()` strings for them. Instead,
`template.typ`'s `node()` function emits them as `<wb-extra-meta>` elements
*inside* the `wb-node` body, where Typst's show rules (including `show link:`)
are active and can render them to proper HTML. The compiler then lifts them out
into metadata so the Jinja template can place them in the right location.

**What HTML difference it fixes:** Two cases:

### 2a. CV node contacts and PDF link

The CV (`0000`) calls `template()` with `contacts: (...)` and
`export-pdf: true`. The contacts contain `#link(...)` and `context` blocks that
resolve differently in paged vs. HTML targets. Without `wb-extra-meta`, the
metadata div is empty:

```html
<!-- v1 -->
<div class="metadata"><ul>
  <li class="meta-item"><span class="link external"><a href="/pdf/0000.pdf">PDF Version</a></span></li>
  <li class="meta-item"><span class="link external"><a href="mailto:guo@hanwen.io">guo@hanwen.io</a></span></li>
  <!-- ... other contacts ... -->
</ul></div>

<!-- v2 without fix -->
<div class="metadata"><ul></ul></div>
```

### 2b. Bibliography nodes when transcluded

Bibliography entries (articles, inproceedings) store their journal/authors/
date/DOI items in `extra-meta` as pre-built HTML strings from
`bibliography-template.typ`. These strings flow correctly when the node is
rendered directly. However, when a bibliography entry is *transcluded*
(e.g. from the CV's Publications section), `transclusion.html` was not
rendering `extra-meta` — only the standard `date` and `authors` metadata fields.
The fix to `transclusion.html` (to check `extra-meta` before falling back to
`date`/`authors`) resolves this; the `wb-extra-meta` mechanism in the compiler
is what makes those strings available in `transclusion.metadata["extra-meta"]`
in the first place.

```html
<!-- v1 (transcluded article) -->
<div class="metadata"><ul>
  <li class="meta-item">Programming 10.2</li>
  <li class="meta-item"><address class="author">Hanwen Guo, Ben Greenman</address></li>
  <li class="meta-item">2025-06-15</li>
  <li class="meta-item"><a class="link external" href="https://doi.org/...">10.22152/...</a></li>
</ul></div>

<!-- v2 without fix -->
<div class="metadata"><ul></ul></div>
```
