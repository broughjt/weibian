# Stage 5 — Metadata, Hardcoded Backmatter, and v1 Parity

This plan supersedes `stage5-plan1.md`. The datalog evaluator, rules parser,
and generic exported-relations mechanism are deferred to Stage 6. Stage 5's
goal is a concrete milestone: **functional parity with Weibian v1**, reproduced
from scratch against the Typst compiler as a library with a working watch mode.

---

## Scope

- Thread arbitrary metadata through the Typst template and into `NodeEntry`
- Visual output: `lang`, taxon span, slug link, metadata div
- Hardcoded backmatter sections matching v1: backlinks, contexts, related,
  references (direct only — no transitive propagation)
- Backmatter rendering pipeline and template
- Watch loop second invalidation axis

What stays deferred to Stage 6:
- Datalog evaluator and rules parser
- User-defined rules file
- `node_meta` ground facts and exported relations
- Transitive reference propagation
- `BackmatterCache` generalised to `HashMap<String, BTreeSet<NodeId>>`

---

## Step 1 — Thread metadata through the Typst template

Extend `#node(identifier, title)` to accept `taxon`, `date`, `tags`, and
`lang` and emit them as attributes on `wb-node`. Same for `#subnode`. Update
the `#template(...)` v1 shim to forward them from its `..args`.

`wb-node` currently emits:

```html
<wb-node identifier="0001">...</wb-node>
```

After this step:

```html
<wb-node identifier="0001" taxon="Post" date="2023-09-22" lang="en">...</wb-node>
```

`date` is serialised to ISO 8601 in Typst
(`datetime.display("[year]-[month]-[day]")`). `tags` is space-separated for
now. `lang` defaults to `"en"` if not specified. Only non-empty values need to
be emitted as attributes.

---

## Step 2 — Extract metadata in `compiler.rs`

`NodeMetadata` in `NodeEntry` is a `HashMap<String, String>`, not a named
struct with fixed fields. This matches the Stage 6 design exactly — the Stage
6 transition just starts asserting these values as `node_meta` facts rather
than reading them through named accessors. No migration needed.

In `extract()`, after parsing the title, iterate remaining attributes on
`wb-node` / `wb-subnode` (excluding `identifier` and `transclude`, which are
structural) and store them verbatim:

```rust
pub metadata: HashMap<String, String>,
```

`tags` is split on whitespace from the single attribute string if needed by a
query, but stored as-is.

---

## Step 3 — Visual output: lang, taxon, slug, metadata div

Pass extracted metadata to the `node.html` template context via `node.meta`:

```rust
minijinja::context! {
    node => minijinja::context! {
        id => name,
        title => entry.title.as_str(),
        title_text => entry.title_text.as_str(),
        body => body,
        meta => &entry.metadata,
    }
}
```

Update `node.html`:

- `<html lang="{{ node.meta.lang | default(value='en') }}">` — was hardcoded
- Inside `<summary><header>`: add a `.taxon` span and `.slug` identifier link
  alongside `<h1>`, matching v1's DOM structure
- Inside `<summary><header>`: add a `.metadata` div containing `node.meta.date`

These are purely template changes once the context is wired up.

---

## Step 4 — `BackmatterCache` and hardcoded queries

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
    pub references: BTreeSet<NodeId>,  // reference nodes I directly link to
}
```

Named fields rather than `HashMap<String, BTreeSet<NodeId>>` — the generic
form is Stage 6 work. Migrating the struct and its diff logic at that point is
a contained change.

Add a backmatter computation pass in `process()` after Pass 2. The queries use
the existing `DiGraphMap`s directly:

```
contexts(id)    = { s | (s → id) ∈ self.transclusions }
backlinks(id)   = { s | (s → id) ∈ self.links }
related(id)     = { t | (id → t) ∈ self.links,
                        nodes[t].metadata.get("taxon") ≠ Some("Reference") }
references(id)  = { t | (id → t) ∈ self.links,
                        nodes[t].metadata.get("taxon") == Some("Reference") }
```

The `references` query is **direct links only** — this matches v1's behaviour
exactly. Transitive reference propagation (where A transcludes B and B cites a
reference, so A's reference list includes it) requires the Stage 6 datalog
evaluator and is explicitly out of scope here.

Compute for every node in `render_order`. The inputs span all nodes even though
only the render set is recomputed — a node outside the dirty set can be the
source of a backlink to a node that is dirty.

---

## Step 5 — Backmatter rendering

Add `backmatter_template` to `weibian.toml` and `BuildConfig`, following the
same pattern as `node_template` and `transclusion_template`.

After the cache computation pass, render a backmatter string per node in the
render set. Resolve `NodeId`s to `{ identifier, title }` objects before passing
to the template — the template should not receive bare IDs:

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

Sections with no items are hidden by the `{% if %}` guard — matching v1 and
forester's `hidden_when_empty` behaviour.

Pass the rendered backmatter string to `node.html` as `node.backmatter` and
add `{{ node.backmatter | safe }}` after the main content block.

---

## Step 6 — Watch loop second invalidation axis

After computing the new `BackmatterCache`, diff against the previous cache.
Any node whose cache changed needs its backmatter re-rendered even if its body
content is unchanged:

```rust
let backmatter_dirty: HashSet<NodeId> = self.nodes.keys()
    .filter(|&&id| {
        self.nodes[&id].backmatter_cache.as_ref()
            != old_backmatter_cache.get(&id).and_then(|c| c.as_ref())
    })
    .copied()
    .collect();
```

Add `backmatter_dirty` to the render set alongside the transclusion-invalidated
set. Nodes only in `backmatter_dirty` can reuse their existing `rendered_body`
— only `rendered_backmatter` needs recomputing. This is the independently-
invalidatable split between the two `Option<String>` fields.

Early exit: if neither the links graph nor the transclusion graph changed
structurally this cycle (no edges added or removed), and no `metadata` maps
changed, skip the backmatter computation pass entirely. This is the common case
when only prose content is edited.

---

## Process loop outline

```
Pass 1  (existing) — anchor rewriting
Pass 2  (existing) — transclusion substitution, collect rendered_body

Pass 3  (new) — backmatter computation
   - contexts:   reverse-walk self.transclusions
   - backlinks:  reverse-walk self.links
   - related:    forward-walk self.links, exclude taxon=Reference
   - references: forward-walk self.links, include only taxon=Reference
   - store in backmatter_cache

Pass 4  (new) — backmatter rendering
   - render backmatter_template per node in render set
   - store in rendered_backmatter

Write   (existing, extended)
   - render node_template with body + backmatter
   - write output files
```

---

## Milestone

After Stage 5, Weibian v2 functionally reproduces Weibian v1:

| Feature | v1 | v2 after Stage 5 |
|---|---|---|
| Full HTML document with CSS + fonts | ✓ | ✓ |
| Watch mode with incremental rebuild | — | ✓ |
| Typst compiler as library | — | ✓ |
| `lang`, taxon, slug, metadata div | ✓ | ✓ |
| Backlinks section | ✓ | ✓ |
| Contexts section | ✓ | ✓ |
| Related section | ✓ | ✓ |
| References section (direct) | ✓ | ✓ |
| References section (transitive) | — | ✗ Stage 6 |
| User-defined datalog rules | — | ✗ Stage 6 |
