# Metadata Design: Multiplicity and Encoding

This document extends `metadata.md` with the analysis of how to handle
multi-valued metadata (multiple authors, multiple tags) correctly. The
`HashMap<String, String>` approach in that document is insufficient — it
cannot represent a document with two authors without an encoding hack.

---

## What forester does

Forester's `frontmatter` record has explicit list-typed fields:

```ocaml
type 'content frontmatter = {
  dates:        Human_datetime.t list;
  attributions: 'content attribution list;  (* authors + contributors *)
  tags:         'content vertex list;
  metas:        (string * 'content) list;   (* arbitrary key-value pairs *)
  taxon:        'content option;
  (* … *)
}
```

Attribution has role information (Author vs Contributor). Tags and author
vertices can be either URI references (links to author profile nodes) or
inline content (literal strings). Metadata values are rich content, not
plain strings.

Key takeaway: forester represents multiplicity at the type level. Two
authors means two elements in the `attributions` list. There is no
comma-encoding or space-encoding.

---

## The problem with HTML attributes

The current design emits metadata as HTML attributes on `<wb-node>`:

```html
<wb-node identifier="0001" taxon="Post" date="2023-09-22" lang="en">
```

HTML attributes are name-value string pairs. A name cannot appear twice on
the same element. So `author="Alice" author="Bob"` is illegal. The only way
to encode multiple values in a single attribute is some delimiter convention
(`author="Alice Bob"`) — which breaks on values containing the delimiter.

Three alternatives for encoding multiple values out of Typst:

1. **Child elements**: `<wb-meta key="author" value="Alice"/>` inside `<wb-node>`
2. **JSON attribute**: `author='["Alice","Bob"]'`
3. **Delimiter encoding**: `author="Alice,Bob"` (fragile)

Child elements are the cleanest: no encoding to parse, no edge cases, and
the Typst `#meta(key, value)` function mirrors forester's `\meta{key}{value}`
syntax directly.

---

## Proposed Typst API

A `#meta(key, value)` function emits a `<wb-meta>` child element inside
the node body. Special-cased convenience arguments (`taxon`, `date`, `lang`)
on `#node` remain for ergonomics but are extracted the same way.

```typst
#show: template(
  identifier: "0001",
  title: [My Post],
  taxon: "Post",
  date: datetime(year: 2024, month: 3, day: 16),
  lang: "en",
)[
  #meta("author", "Alice Smith")
  #meta("author", "Bob Jones")
  #meta("tag", "rust")
  #meta("tag", "programming")
  …content…
]
```

Emitted HTML:

```html
<wb-node identifier="0001" taxon="Post" date="2024-03-16" lang="en">
  <wb-meta key="author" value="Alice Smith"/>
  <wb-meta key="author" value="Bob Jones"/>
  <wb-meta key="tag" value="rust"/>
  <wb-meta key="tag" value="programming"/>
  …content…
</wb-node>
```

The extractor in `compiler.rs` collects all `<wb-meta>` children into
the metadata store, then removes them before rendering the body.

---

## Rust data type options

### Option A — `Vec<(String, String)>` (ordered pairs, duplicate keys)

```rust
pub metadata: Vec<(String, String)>,
```

- Multiple authors: `[("author", "Alice"), ("author", "Bob")]`
- Maps 1:1 to datalog `node_meta` facts with no transformation
- Preserves declaration order
- Downside: template access requires grouping — `node.meta.taxon` doesn't
  work directly

### Option B — `HashMap<String, Vec<String>>`

```rust
pub metadata: HashMap<String, Vec<String>>,
```

- Multiple authors: `{ "author": ["Alice", "Bob"] }`
- Template access is natural: `node.meta.taxon[0]`, `for tag in node.meta.tag`
- For datalog facts: iterate outer map, then inner vec — one fact per value
- Downside: HashMap doesn't preserve key insertion order

### Option C — Vec<(String, String)> stored, grouped view for templates

Store `Vec<(String, String)>` internally. When building the template
context, pre-compute a `HashMap<String, Vec<String>>` grouped view.
Templates see the grouped form; internal code (datalog fact assembly)
sees the flat list.

Best of both worlds, with one conversion step at render time.

### Option D — Typed known fields + generic extras (forester's approach)

```rust
pub struct NodeMetadata {
    pub taxon:   Option<String>,
    pub date:    Option<String>,
    pub lang:    Option<String>,
    pub authors: Vec<String>,
    pub tags:    Vec<String>,
    pub extra:   Vec<(String, String)>,
}
```

- First-class template access for known fields
- Downside: compiler now has semantic knowledge of specific fields,
  which violates the principle in `metadata.md`. `extra` also still
  has the multiplicity problem for user-defined multi-valued keys.

---

## Recommended approach

**Option B (`HashMap<String, Vec<String>>`) or Option C**, with
`<wb-meta>` child elements for the Typst/HTML encoding.

Option B is simpler: one type, natural template access, straightforward
datalog fact assembly. If key order in templates matters (e.g. tags
rendering in declaration order), use `indexmap::IndexMap<String, Vec<String>>`
instead of `HashMap` — it's a small dependency with a drop-in API.

Option C is appealing if the flat-list form is ever needed directly, but
the conversion is trivial enough that it can be added later if a concrete
need arises.

Option A is the right choice if the primary consumer is the datalog
engine and template ergonomics are less important. This is worth
reconsidering once Stage 6 design is more concrete.

---

## Template ergonomics with `HashMap<String, Vec<String>>`

```jinja
{# Single-valued fields — take first element #}
{% if node.meta.taxon %}<span class="taxon">{{ node.meta.taxon[0] }}</span>{% endif %}
{% if node.meta.date %}<time>{{ node.meta.date[0] }}</time>{% endif %}
<html lang="{{ node.meta.lang[0] | default(value="en") }}">

{# Multi-valued fields — iterate #}
{% if node.meta.author %}
<ul class="authors">
  {% for author in node.meta.author %}
  <li>{{ author }}</li>
  {% endfor %}
</ul>
{% endif %}
```

The `[0]` access for single-valued fields is slightly verbose. A helper
filter like `| first` would clean this up:

```jinja
<html lang="{{ node.meta.lang | first | default(value="en") }}">
```

MiniJinja supports `first` as a built-in filter.

---

## Datalog ground facts

With `HashMap<String, Vec<String>>` (or `Vec<(String, String)>`), ground
fact assembly is straightforward:

```rust
for (key, values) in &node.metadata {
    for value in values {
        db.assert(node_meta_rel, &[node_id, intern(key), intern(value)]);
    }
}
```

A document with two authors produces two `node_meta` facts:

```
node_meta(0001, "author", "Alice Smith")
node_meta(0001, "author", "Bob Jones")
```

User rules can then derive relations over authors:

```datalog
has_author(X, Y) :- node_meta(X, "author", Y).
coauthored(X, Z) :- has_author(X, A), has_author(Z, A), X != Z.
```

---

## Open question

`HashMap<String, Vec<String>>` vs `Vec<(String, String)>` is the main
unresolved choice. The deciding factors:

- If key order across the map matters for template rendering: use
  `IndexMap` (Option B with insertion-order preservation)
- If declaration order of individual values matters but not key order:
  both work (Vec always preserves order within a key)
- If the flat list form is significantly better for Stage 6 datalog
  implementation: prefer Option A or C
- If template ergonomics should drive the decision: Option B is cleanest
