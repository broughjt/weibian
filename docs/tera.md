# Tera Templates

## Templates

There are four templates in `.wb/templates/`:

- `note.html` — full page wrapper
- `transclusion.html` — renders a single transcluded node
- `internal_link.html` — renders `<wb-internal-link>` as an `<a>` tag
- `citation.html` — renders `<wb-cite>` as an `<a class="citation">` tag

## How they are called

Tera is loaded once from `.wb/templates/**/*.html`. Two custom filters are registered on
the Tera instance at startup. Then `render_template(&templates, "name.html", &context)`
is called at the appropriate point during node rendering. The templates are loaded from
the user's project directory, not embedded in the binary.

## Template details

### `transclusion.html`

The most interesting template. Receives a `transclusion` context object:

```
transclusion.content          // already-rendered HTML string
transclusion.show_metadata    // bool
transclusion.expanded         // bool — whether <details> is open
transclusion.disable_numbering// bool
transclusion.demote_headings  // int — levels to bump h1→h2 etc.
transclusion.metadata.lang    // string
transclusion.metadata.title   // string (optional)
```

Before doing anything structural it applies two custom Rust-registered Tera filters to
the content HTML:

- `wb_disable_numbering` — adds a CSS class to all headings to suppress counter
  rendering
- `wb_demote_headings(levels=N)` — bumps heading levels by N (h1→h2, h2→h3, etc.)

Both filters parse and mutate the HTML DOM. They are the reason Tera is involved in
transclusion rendering at all — the structural wrapping alone wouldn't need a template
engine, but the heading manipulation does.

The template then wraps the result in:

```html
<section class="block">
  <details open?> ... </details>
</section>
```

### `note.html`

Assembles the full output page. Receives:

```
note.id
note.head                    // <head> HTML from Typst output
note.content                 // transclusion-resolved body HTML
note.metadata.lang
note.metadata.toc            // "false" to suppress TOC
note.metadata.title
note.backmatter_sections     // Vec<{ title: String, content: String }>
note.toc                     // nested heading structure (see below)
```

`note.backmatter_sections` is a `Vec<{title, content}>` where `content` is
already-rendered HTML. The template iterates the sections and applies
`wb_disable_numbering | wb_demote_headings(levels=1)` to each content string.

The TOC is a nested structure of heading objects built from the rendered body HTML.
Each heading has `id`, `content`, `disable_numbering`, and `children`.

### `internal_link.html`

Trivial — renders `<a href="{{ link.href }}">{{ link.text }}</a>`.

### `citation.html`

Trivial — renders `<a href="{{ citation.href }}" class="citation">{{ citation.text }}</a>`.

---

## Custom Tera filters

Both filters are implemented in v1's `backend.rs` (lines 949–1012) and are
self-contained. They need to be re-registered in v2:

```rust
tera.register_filter("wb_disable_numbering", wb_disable_numbering_filter);
tera.register_filter("wb_demote_headings", wb_demote_headings_filter);
```

---

## Implications for v2

**The template interface is worth keeping as-is.** The context shapes, filter names, and
file layout can all carry over directly.

**Backmatter rendering.** In v1, `build_backmatter_sections` creates a virtual note full
of `<wb-transclusion>` elements and runs it through the rendering pipeline, so by the
time `note.html` sees `backmatter_sections` the content is finished HTML. In v2 the
mechanism is different — datafrog produces `BTreeSet<NodeId>` results per node, and
rendering a backmatter section means looking up each node's `rendered_body` from the
NodeStore and passing it through `transclusion.html` — but the output shape passed to
`note.html` stays identical.

**`NodeMetadata` must carry `lang` and `toc`** (at minimum) for the `note.html` template
context to work.

**Filter re-registration.** The two custom filters need to be registered on the Tera
instance before any rendering happens. They are self-contained and can be lifted
directly from v1's `backend.rs`.
