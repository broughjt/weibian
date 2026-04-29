# Quartz Investigation Notes

[Quartz](https://quartz.jzhao.xyz/) is a static site generator that builds a website from Obsidian markdown notes. These notes document its architecture and how its SPA + graph view could be adapted for Weibian.

## Quartz Architecture

### Frontend framework: Preact (server-side only)

Quartz uses Preact (a lightweight React alternative) configured via `"jsxImportSource": "preact"` in `tsconfig.json`. However, Preact is only used **at build time** — components are rendered to HTML strings via `preact-render-to-string`. The resulting site is static HTML; Preact does not run in the browser.

### SPA navigation

Despite being a static site, Quartz implements single-page app navigation. `spa.inline.ts` (~200 lines of vanilla TypeScript, no framework) intercepts all link clicks, fetches the target page as HTML via `fetch()`, then patches `document.body` using `micromorph` (a morphdom-style DOM diffing library). URLs are updated with `history.pushState()`. Back/forward buttons are handled via `popstate`. A custom `"nav"` event is dispatched after each navigation so other components can react.

This approach works against any static HTML output — the fetched page is a full HTML document; the router extracts `html.body` and morphs it in place.

### Graph view

No backend API. The graph uses entirely static data:

1. **Data generation (build time):** The `contentIndex` emitter plugin serializes all page slugs, titles, and link relationships to `static/contentIndex.json`.

2. **Data injection (per-page):** Each rendered page includes an inline `<script>` that creates a global `fetchData` promise:
   ```js
   const fetchData = fetch("/static/contentIndex.json").then(data => data.json())
   ```

3. **Rendering (client-side):** The graph component awaits `fetchData`, then uses:
   - **D3** for force-directed node layout (physics simulation)
   - **Pixi.js** (WebGL) for high-performance rendering of nodes and edges
   - **Tween.js** for animations

   Clicking a node calls `window.spaNavigate(...)` to navigate via the SPA router.

The graph component is plain TypeScript — not a React/Preact component.

---

## Adapting for Weibian

### Feasibility

Yes, straightforward. The SPA router and graph view are client-side TypeScript with no dependency on Quartz's Preact SSR pipeline. They can be adapted and bundled independently.

### What the Rust pipeline needs

**One addition:** emit `dist/static/contentIndex.json` during the `process()` step. Weibian already has all the required data in memory — node IDs, titles, and the link/transclusion graph in `petgraph`. This is a small serialization step.

### What the `node.html` template needs

Two `<script>` tags:

```html
<!-- inject the data promise -->
<script>const fetchData = fetch("/static/contentIndex.json").then(r => r.json())</script>
<!-- load the bundled frontend code -->
<script type="module" src="/static/bundle.js"></script>
```

### Frontend code

Quartz's `spa.inline.ts` and `graph.inline.ts` can be adapted almost directly:

- **SPA router:** Works against Weibian's existing full-page HTML output without changes to the Rust pipeline.
- **Graph renderer:** Wire `fetchData` to Weibian's `contentIndex.json`; the D3 + Pixi.js rendering code is self-contained.

### JS build tooling

Weibian currently has no JavaScript build step. The simplest option is **esbuild** — a single binary, no config required, and what Quartz itself uses. It can be invoked from the Rust watch loop or run as a separate step. Since the frontend code doesn't change with content, it can also be pre-bundled and treated as a static asset.
