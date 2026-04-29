# Table of Contents

## Background

Weibian v1 and Forester both support a table of contents sidebar, but they
derive it differently.

**Forester** builds the TOC directly from the document's abstract content tree.
Its language has sections as a first-class concept (`T.Section`), so TOC
structure falls naturally out of document structure — `render_toc` just
recurses over `section.mainmatter` nodes. No HTML scraping involved.

**Weibian v1** post-processes rendered HTML: after Typst compiles a node to
HTML, the compiler parses the output for `h1`–`h6` elements (using a CSS
selector), extracts their `id`, text content, and `disable-numbering` class,
then builds a nested `Vec<Heading>` tree which is passed to the Tera template
as `note.toc`. The template renders the sidebar by iterating over up to five
levels of nesting.

A single node with no transclusions can have a non-trivial TOC — headings are
just document structure within a node's body, unrelated to node IDs.

## Status in v2

Not yet implemented. The v2 Jinja templates do not render a TOC sidebar.

## Options for v2

Since Typst gives us rendered HTML output rather than a structured content tree
we control, the two realistic options are:

**Option A: HTML scraping (v1's approach)**
After rendering each node's body, parse the HTML for headings, build a nested
tree, and pass it to the Jinja template as `node.toc`. This is straightforward
to implement (we already have `dom_query` in the compiler) and directly follows
v1's precedent. The downside is that it's a post-processing step on HTML rather
than a derivation from document structure.

**Option B: Typst AST extraction**
Hook into Typst's rendering pipeline to extract heading structure before HTML
serialization. This would be more in the spirit of Forester's approach — TOC
comes from document structure, not HTML — but requires deeper Typst integration
and is considerably more involved.

## Recommendation

Option A is the pragmatic path. It matches v1's behavior, reuses existing
infrastructure, and can be implemented without changes to the Typst layer. The
TOC is a display concern and HTML scraping is a well-understood technique for
it.

The longer-term ideal (closer to Forester) would be for the Typst template to
emit structured heading metadata (e.g. via `#metadata`) alongside the body,
giving the compiler a clean structured source for TOC without HTML parsing. But
this requires changes to the Typst template and is a larger undertaking.
