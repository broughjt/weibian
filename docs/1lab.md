# 1lab Investigation Notes

[1lab](https://1lab.dev) is a project that renders literate Agda as a website with interactive type information on hover. These notes document its architecture and how a similar capability could be supported in Weibian via a hooks mechanism.

## How 1lab implements Agda type processing

### Build system

1lab uses a custom **Shake** build system written in Haskell (`1lab-shake`). The Agda compiler is not a separate tool — it is imported directly as a Haskell library dependency, so the Shake binary embeds the Agda typechecking infrastructure.

Watch mode is supported via `1lab-shake all -w`. It uses `fsnotify` to monitor `src/` and `support/`. When Agda files change it targets only the changed modules' HTML output for rebuild; when non-Agda files change it falls back to a full rebuild. Shake tracks dependency digests so only stale targets are recomputed.

### Type extraction

Type information is extracted **at build time during Agda typechecking**, not queried from a live process. A custom Agda backend (`HTML.Backend`) hooks into the typechecking phase, processes each definition, normalises its type, reifies it to an AST, and renders it as HTML. The result is serialised to per-module JSON files at `_build/html/types/<Module>.json`.

The JSON is an array indexed by integer file position. Each entry is an HTML-formatted string:

```json
{
  "0": "<pre class=\"Agda\"><span class=\"Function\">f</span> <span class=\"Symbol\">:</span> <span class=\"PrimitiveType\">Set</span></pre>",
  "42": "..."
}
```

The key data structures:

```haskell
data Identifier = Identifier
  { idIdent   :: Text   -- identifier name
  , idAnchor  :: Text   -- anchor in rendered HTML
  , idType    :: Text   -- plain text type
  , idTooltip :: Text   -- HTML-formatted type for display
  }

data HtmlModule = HtmlModule
  { htmlModIdentifiers :: IntMap Identifier  -- file position → identifier
  , htmlModImports     :: [String]
  }
```

### Highlighted HTML output

The Agda backend also produces **semantically highlighted HTML** directly. Each token is wrapped in an `<a>` tag with:
- A numeric `id` attribute set to the token's file position
- CSS classes for the token kind (Function, Datatype, Symbol, etc.)
- A `data-type="true"` attribute when type information is available
- An `href` of the form `Module.html#<position>`

This means the highlighted HTML and the type JSON are both produced in the same compilation step.

### Frontend

The frontend is vanilla TypeScript (~80 lines, no framework). On hover over an `<a data-type="true">` element:

1. The module name and position are extracted from the `href`
2. `types/<module>.json` is fetched (result cached in a `Map` after first load)
3. `types[position]` gives the pre-rendered HTML type string
4. A popup `div` is created, positioned relative to the viewport, and faded in

A second hover variant handles wiki-links by fetching pre-rendered HTML fragments from `fragments/<id>.html`.

The frontend is bundled with esbuild.

---

## Applying this to Weibian

### The source format

Weibian notes are written as `.lagda.typ` files — literate Agda where Typst is the documentation language. Agda code lives in ` ```agda ... ``` ` blocks; the surrounding text is Typst. The same file is valid both as Typst input and as an Agda module.

When Weibian compiles a `.lagda.typ` file via Typst, the Agda code blocks come out as plain `<pre><code class="language-agda">...</code></pre>`. The content is already in the right place in the output — no transclusion of separately-generated fragments is needed.

### What needs to happen additionally

Two things need to be produced that Typst cannot provide:

1. **Semantic highlighting** — replacing the plain code blocks with token-coloured, definition-linked HTML (as the Agda backend produces in 1lab)
2. **Type sidecar JSON** — `dist/types/<Module>.json` files for the frontend to fetch on hover

Both are produced by running the Agda compiler on the same `.lagda.typ` files that Weibian already compiled. In 1lab these happen in one step because the build system embeds the Agda compiler. In Weibian they would be a separate post-processing step.

### A post-processing hook

The cleanest model is a **post-processing hook**: after Weibian writes HTML files to `dist/`, a hook receives the list of files that were just rebuilt and transforms them in-place (replacing plain code blocks with highlighted versions) while also emitting the type JSON sidecars.

Crucially, the hook must receive the **list of changed output files** rather than being triggered blindly, so it only re-runs the Agda compiler on modules that actually changed. Agda typechecking is slow; running it on everything on every watch iteration would be unacceptable.

A sketch of the interface:

```toml
[[hook]]
name = "agda-highlight"
on_start = "agda-highlighter --watch --output dist/types/"
# or, for one-shot per build:
post_build = "agda-highlighter"
# Weibian passes changed output paths as arguments or on stdin
```

The hook tool would:
1. Receive `dist/Base.Identity.Core.html dist/Base.Function.Negation.html` (the files Weibian just rebuilt)
2. Locate the corresponding `.lagda.typ` source files
3. Run the Agda compiler (using its own `.agdai` cache for incremental typechecking)
4. Replace `<pre><code class="language-agda">` blocks with semantically highlighted HTML
5. Write `dist/types/Base.Identity.Core.json` etc.

### Two hook variants

This use case motivates two hook flavors:

| | `post_build` (one-shot) | `on_start` (background) |
|---|---|---|
| Ordering | Guaranteed: Weibian waits for hook before next iteration | Eventual: hook catches up asynchronously |
| Incremental speed | Re-invoked per build; tool manages its own cache (`.agdai`) | Tool runs its own watch loop |
| Good for | Post-processing with fast external tools | Slow compilers (Agda, TypeScript) |

For Agda, `on_start` with a background watcher is preferable because Agda typechecking benefits from keeping the compiler process alive across iterations (warm interface file cache). The tradeoff is that on first load the page may briefly show un-highlighted code before the Agda watcher catches up.

### Nothing needs to be special-cased in Weibian core

The Agda use case does not require Weibian to know anything about Agda. Weibian only needs to:
- Support a `post_build` or `on_start` hook configuration
- Pass the list of rebuilt output file paths to `post_build` hooks

The hook tool is entirely user-supplied. The same hook mechanism covers other post-processing use cases (minification, link checking, etc.).
