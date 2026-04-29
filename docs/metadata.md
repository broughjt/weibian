# Generic Metadata Design

## The principle

The compiler should have no semantic knowledge of metadata fields. It should
not know what "taxon" means, what constitutes a "Reference", or how "author"
relates to contributor inheritance. All of that belongs in user-defined datalog
rules and Jinja templates. The compiler's only job is passing metadata through.

---

## What the compiler contributes

Three ground fact types, nothing else:

```
transcludes(X, Y)            -- from the transclusion graph
links_to(X, Y)               -- from the links graph
node_meta(X, "key", "value") -- one fact per wb-node attribute
```

`NodeMetadata` in `NodeEntry` is a `HashMap<String, String>`, not a named
struct. `extract()` iterates the attributes on `wb-node` / `wb-subnode` and
stores them verbatim. Only attributes beyond `identifier` and `transclude` are
collected — those are structural, not metadata.

The Typst template's job is to forward all metadata keyword arguments as
attributes on `wb-node`. Anything the author writes:

```typst
#show: template(
  title: [Gödel's β Function],
  identifier: "0001",
  taxon: "Post",
  date: datetime(year: 2023, month: 9, day: 22),
  tags: ("math",),
  lang: "en",
)
```

...becomes attributes on the emitted element:

```html
<wb-node identifier="0001" taxon="Post" date="2023-09-22" lang="en" tags="math">
```

`date` is serialised to ISO 8601 in Typst. `tags` is space-separated. The
compiler reads these as plain strings and asserts one `node_meta` fact per
attribute. It never interprets any of them.

---

## User-defined rules

A `datalog_rules` path in `weibian.toml` points to a rules file the user
writes. The compiler provides the ground facts; the user builds everything else:

```datalog
is_reference(X) :- node_meta(X, "taxon", "Reference").
is_person(X)    :- node_meta(X, "taxon", "Person").

backlinks(X, Y)  :- links_to(Y, X).
contexts(X, Y)   :- transcludes(Y, X).
related(X, Y)    :- links_to(X, Y), not is_reference(Y).

transcludes_tc(X, Z)  :- transcludes(X, Y), transcludes_tc(Y, Z).
transcludes_rtc(X, X) :- transcludes(X, _).
transcludes_rtc(X, Y) :- transcludes_tc(X, Y).

references(X, Z) :- transcludes_rtc(X, Y), links_to(Y, Z), is_reference(Z).
```

Nothing here is hardcoded in the compiler. A user who has no "Reference" taxon
simply omits those rules. A user who wants a custom "Prerequisite" relation
adds it themselves.

---

## Exported relations

The user also declares which computed relations they want available in
templates. In `weibian.toml`:

```toml
[datalog]
rules = "datalog/rules.dl"
export = ["backlinks", "contexts", "related", "references"]
```

After the fixed-point, the compiler resolves each exported relation's node IDs
to `{ identifier, title, meta }` objects and makes them available in the
template context. Only exported relations are resolved — computing everything
for all relations would be wasteful.

---

## Template side

`node.html` receives `node.meta` as the raw key-value map, and can reference
any field directly:

```jinja
{% if node.meta.taxon %}<span class="taxon">{{ node.meta.taxon }}</span>{% endif %}
{% if node.meta.date %}<time>{{ node.meta.date }}</time>{% endif %}
<html lang="{{ node.meta.lang | default(value="en") }}">
```

The backmatter template receives the exported relations as lists of resolved
node objects:

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
```

The template author decides which exported relations to render and how. Empty
sections are hidden by the `{% if %}` guard.

---

## Watch loop implications

`BackmatterCache` becomes `HashMap<String, BTreeSet<NodeId>>` keyed by
relation name, rather than a named struct with fixed fields. The diffing logic
is identical — compare new results against cached results per relation, collect
nodes with any changed result set into the backmatter re-render set. The
"no facts changed" early exit still applies: if neither the link graph nor the
transclusion graph changed structurally this cycle, and no `node_meta` facts
changed, skip the datalog pass entirely.

---

## The datalog evaluator

The rule language needs are narrow enough to write a small focused evaluator
rather than taking on a large dependency. Required features:

- Joins on small in-memory relations
- Recursive rules for transitive closure (semi-naive evaluation to avoid
  redundant recomputation)
- Stratified negation for `not is_reference(Y)` style filters
- A simple text parser for the `:- ` rule syntax

This is roughly 300–400 lines of Rust with no new dependencies. See
`egglog.md` for the analysis of why existing libraries were ruled out.
