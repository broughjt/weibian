# Stage 5 — Metadata, Backmatter, and Visual Output

This document covers the implementation plan for Stage 5. The goal is metadata
extraction, backmatter sections (backlinks, contexts, related, references), and
the visual output improvements deferred from Stage 4 (lang, taxon, slug, metadata
div).

---

## Starting point: the Typst template is the bottleneck

The source files already pass `taxon:`, `date:`, `tags:` to `#template(...)` —
for example, `0001` has `taxon: "Post"` and `date: datetime(...)`. But
`#template()` and `#node()` currently swallow all of those. `wb-node` only emits
`identifier`. Every Stage 5 concern is blocked on threading metadata through the
Typst template first.

The graphs are already built. `self.links` and `self.transclusions` in `Compiler`
are `DiGraphMap`s with all the right edges. Contexts (reverse transclusion) and
backlinks (reverse links) are pure reverse-graph lookups requiring zero new
infrastructure once metadata is available to distinguish reference nodes.

`datafrog` is already a dependency, so the references pass has no additional
Cargo cost.

The watch loop second axis (backmatter invalidation) is non-trivial. Get
backmatter correct on full builds first, then make it incremental.

---

## Step 1 — Thread metadata through the Typst template

Extend `#node(identifier, title)` to accept `taxon`, `date`, `tags`, and `lang`,
and forward them as attributes on `wb-node`. Same for `#subnode`. Update the
`#template(...)` v1 shim to forward them.

`wb-node` currently emits:

```html
<wb-node identifier="0001">...</wb-node>
```

After this step:

```html
<wb-node identifier="0001" taxon="Post" date="2023-09-22" lang="en">...</wb-node>
```

`date` is serialised to an ISO 8601 string in Typst
(`datetime.display("[year]-[month]-[day]")`). `tags` can be a space-separated
string for now. `lang` defaults to `"en"` if not specified. Only non-empty values
need to be emitted as attributes — absent attributes are treated as absent
metadata, not errors.

---

## Step 2 — Define `NodeMetadata` and extract it in `compiler.rs`

Add to `compiler.rs`:

```rust
#[derive(Default, Clone)]
pub struct NodeMetadata {
    pub taxon: Option<String>,
    pub date: Option<String>,
    pub lang: Option<String>,
    pub tags: Vec<String>,
}
```

Add `metadata: NodeMetadata` to `NodeEntry`. In `extract()`, after parsing the
`wb-node` / `wb-subnode` element and its title, read the attributes into
`NodeMetadata`. The attribute names are the same ones emitted in Step 1.

`tags` is split on whitespace from the single attribute string. All other fields
are taken as-is from their attribute values.

---

## Step 3 — Visual output: lang, taxon, slug, metadata div

Pass the extracted metadata to the `node.html` template context:

```rust
minijinja::context! {
    node => minijinja::context! {
        id => name,
        title => entry.title.as_str(),
        title_text => entry.title_text.as_str(),
        body => body,
        taxon => entry.metadata.taxon.as_deref().unwrap_or(""),
        date => entry.metadata.date.as_deref().unwrap_or(""),
        lang => entry.metadata.lang.as_deref().unwrap_or("en"),
    }
}
```

Update `node.html`:

- `<html lang="{{ node.lang }}">` — currently hardcoded `"en"`.
- Inside the `<summary><header>`: add a `.taxon` span and `.slug` identifier link
  alongside the `<h1>`, matching v1's structure.
- Inside the `<summary><header>`: add a `.metadata` div containing `node.date`.

These are purely template changes after the context is wired up correctly.

---

## Step 4 — `BackmatterCache` and simple queries

Add to `NodeEntry`:

```rust
pub backmatter_cache: Option<BackmatterCache>,
```

```rust
#[derive(Default, Clone, PartialEq, Eq)]
pub struct BackmatterCache {
    pub contexts: BTreeSet<NodeId>,    // nodes that transclude me
    pub backlinks: BTreeSet<NodeId>,   // nodes that link to me
    pub related: BTreeSet<NodeId>,     // nodes I link to, excluding references
    pub references: BTreeSet<NodeId>,  // filled in Step 5
}
```

Add a backmatter computation pass in `process()`, after Pass 2 (transclusion
rendering). The three simple queries are reverse-graph lookups on the
already-built `DiGraphMap`s:

```
contexts(id)  = { s | (s → id) ∈ self.transclusions }
backlinks(id) = { s | (s → id) ∈ self.links }
related(id)   = { t | (id → t) ∈ self.links, nodes[t].metadata.taxon ≠ Some("Reference") }
```

Compute and store in `backmatter_cache` for every node in `render_order`.
`references` is left as `BTreeSet::new()` until Step 5.

Only nodes in `render_order` need their backmatter recomputed on a given
`process()` call — but the *input* to the queries (the full reverse-graph) spans
all nodes, not just the render set. The contexts/backlinks of a re-rendered node
may come from nodes that were not themselves dirty this cycle.

---

## Step 5 — datafrog references pass

After the backmatter computation, run a datafrog pass for the `references`
relation: nodes with `taxon == "Reference"` reachable from a given node by
transitively following transclusion edges and then following one link edge.

Formally (mirroring forester's axioms):

```
transcludes_tc(x, z) :- transcludes(x, y), transcludes_tc(y, z).
transcludes_rtc(x, x).
transcludes_rtc(x, y) :- transcludes_tc(x, y).
references(x, z) :- transcludes_rtc(x, y), links_to(y, z), is_reference(z).
```

`NodeId(u32)` maps directly to datafrog's `u32` key type — no additional
interning is needed. Build the input `Relation`s by iterating `self.nodes`:

```rust
let transcludes: Relation<(u32, u32)> = self.nodes.values()
    .flat_map(|e| {
        let src = e.id.0;
        self.transclusions.neighbors(e.id).map(move |t| (src, t.0))
    })
    .collect();

let links_to: Relation<(u32, u32)> = /* same pattern over self.links */;

let is_reference: Relation<(u32, ())> = self.nodes.values()
    .filter(|e| e.metadata.taxon.as_deref() == Some("Reference"))
    .map(|e| (e.id.0, ()))
    .collect();
```

After the fixed-point, write `references` results back into each node's
`backmatter_cache`.

---

## Step 6 — Backmatter rendering

Add `backmatter_template` to `weibian.toml` and `BuildConfig`, following the
same pattern as `node_template` and `transclusion_template`.

After the backmatter computation and datafrog pass, render a backmatter string
per node using the template. The template receives resolved `(identifier, title)`
pairs, not bare `NodeId`s, so `process()` must look up titles before passing
context:

```jinja
{% if backmatter.backlinks %}
<section class="block">
  <details>
    <summary><header><h1>Backlinks</h1></header></summary>
    <ul>
      {% for node in backmatter.backlinks %}
      <li><a href="/{{ node.identifier }}.html">{{ node.title | safe }}</a></li>
      {% endfor %}
    </ul>
  </details>
</section>
{% endif %}
{# same pattern for contexts, related, references #}
```

Sections with no items are hidden by the `{% if %}` guard, matching v1 and
forester's `hidden_when_empty` behaviour.

Pass the rendered backmatter string to `node.html` as `node.backmatter` and add
`{{ node.backmatter | safe }}` after the main content block.

---

## Step 7 — Watch loop second invalidation axis

After computing the new `BackmatterCache` for all nodes, diff against the
previous cache. Nodes whose cache changed need their backmatter re-rendered, even
if their body content is unchanged:

```rust
let backmatter_dirty: HashSet<NodeId> = self.nodes.keys()
    .filter(|&&id| {
        self.nodes[&id].backmatter_cache != old_backmatter_cache.get(&id)
    })
    .copied()
    .collect();
```

Add `backmatter_dirty` to the render set. Nodes that are only in
`backmatter_dirty` (not in the transclusion re-render set) only need their
backmatter re-rendered — `rendered_body` can be reused. This is the split between
the two `Option<String>` fields on `NodeEntry`: they are independently
invalidatable.

Early exit: if the links graph and transclusion graph are structurally unchanged
from the previous iteration (no edges added or removed), skip the datafrog pass
and the backmatter diff entirely. This is the common case when only prose content
is edited.

---

## Process loop outline

```
Pass 1  (existing) — anchor rewriting + transclusion substitution
Pass 2  (existing) — collect rendered_body

Pass 3  (new) — backmatter computation
   - Compute contexts, backlinks, related via reverse-graph lookups
   - Run datafrog pass for references
   - Store results in backmatter_cache

Pass 4  (new) — backmatter rendering
   - Render backmatter_template for each node in render set
   - Store result in rendered_backmatter

Write   (existing, extended)
   - Render node_template with body + backmatter
   - Write output files
```

---

## What this achieves

| Feature | After Stage 5 |
|---|---|
| `lang` attribute on `<html>` | ✓ |
| `.taxon` span, `.slug` link, `.metadata` date div | ✓ |
| Backlinks section | ✓ |
| Contexts (transclusion parents) section | ✓ |
| Related section | ✓ |
| References (transitive, via datafrog) | ✓ |
| Backmatter hidden when empty | ✓ |
| Watch loop backmatter invalidation | ✓ |
| TOC sidebar | ✗ (no design yet) |
| Contextual numbering | ✗ (deferred) |
