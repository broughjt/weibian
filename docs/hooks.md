This seems right to me. Even if someone is not using Shake, it's a good thing to have the build caching extend between runs of the binary. I
know many compilers do this.
    
Thinking more about how the Agda html thing would work specifically, I'm wondering if we don't still need some sort of hook system in Weibian.
We support arbitrary transclusion, so the same Agda code could show up in several different output pages. How will the post processing command
know which code it's looking at?
    
You've hit on something important. Let me think it through.
    
The problem: node Base.Identity.Core contains Agda code. Nodes PageA, PageB, PageC all transclude it. After Weibian runs, that same Agda code appears
in three output pages. A page-level post-processor receiving dist/PageA.html has lost the information that the code came from Base.Identity.Core —
it can't easily know which source file to pass to the Agda compiler, and if it figures it out, it'll redundantly reprocess the same source three
times.
    
The solution is to run the hook at the node level, not the page level.
    
Weibian already has a natural seam between two phases:
1. Compile phase — .lagda.typ → raw per-node HTML (with plain code blocks)
2. Process phase — resolve transclusions, assemble pages, write output
   
A hook between these two phases receives individual node HTML, transforms it, and then Weibian assembles the pages using the already-transformed
HTML. When PageA, PageB, PageC transclude Base.Identity.Core, they all get the highlighted version. The Agda compiler runs once per source node, not
once per output page. The type JSON is unambiguously types/Base.Identity.Core.json regardless of where the content ends up.
    
This is actually exactly how 1lab works — highlighting happens per-module during compilation, before any page assembly.
    
So yes, Weibian does need a hook system, but the interface is narrower than "post-process this output HTML page": it's "transform this node's HTML
before transclusion is resolved". Something like:
    
[[hook]]
name = "agda-highlight"
run = "agda-highlighter"
# Weibian passes: node id, source file path, raw html path
# Hook rewrites the html in-place and emits sidecar files to dist/
    
Weibian passes the affected node identities (after compile, before process), the hook transforms them, and then process runs on the enriched HTML.
This also composes naturally with the disk cache — the transformed node HTML can itself be cached per-node, so unchanged nodes skip the hook
entirely.

Would it make sense to allow hooks to run before `extract`, after extract on the individial nodes, and after render as post-processing?

Yes, that structure maps cleanly onto the natural seams in the pipeline:
    
.typ source
↓  compile (Typst)
raw HTML document
↓  [pre-extract hook]
↓  extract
individual node HTML fragments
↓  [post-extract hook, per node]
↓  process (transclusion, backmatter, template)
rendered page HTML
↓  [post-render hook, per page]
output
    
Post-extract is the key one, as established — it's where Agda highlighting belongs. Each node corresponds to a source file, transclusion hasn't
happened yet, each node is processed once.
    
Post-render is the natural place for classic post-processing: minification, link checking, anything that needs to see the fully-assembled templated
page including backmatter.
    
Pre-extract is the weakest case. I'm struggling to think of a compelling use case that isn't better handled either in Typst itself (since Typst can
emit arbitrary HTML elements) or at the post-extract level. Do you have something in mind for it?
    
One thing worth noting for the cache design: hooks participate in cache invalidation. If a post-extract hook's configuration or binary changes, all
cached post-extract results are stale and need to be rerun. Weibian would need to store a fingerprint of each hook's config alongside the cached node
HTML.
    
Well Agda highlighting would need to run as a pre-extract hook, not a post-extract hook. There can be several nodes per typst source file, so
we'd want to run Agda highlighting while we still had access to Agda source as a whole module and not fragments of a module.
    
This doesn't seem too hard. We just let the user specify three binaries and figure out a sane protocol for passing them the information they
need to run. Is this a good idea?
    
Yes, this is a good idea. Simple, language-agnostic, composable — and the three stages now have clear motivations:
    
- Pre-extract: whole compiled document, 1-to-1 with source file. Right for Agda since the module boundary is the file boundary.
    - Post-extract: individual node fragments, before transclusion. Right for per-node enrichment where you don't need whole-module context.
- Post-render: fully assembled pages. Right for minification, link checking, analytics injection, etc.
  
      One protocol decision worth thinking through upfront: per-file invocation vs batch invocation. If Weibian calls the hook once per node/page, spawning
a new process each time, you pay process startup cost for every node. With hundreds of nodes that's noticeable even if the hook itself is fast. The
simpler fix is to invoke the hook once per build iteration with all affected paths as arguments:
    
pre-extract-hook typ/Base/Identity/Core.lagda.typ _cache/Base.Identity.Core.html \
typ/Base/Function/Negation.lagda.typ _cache/Base.Function.Negation.html ...
    
The hook processes all affected files in one invocation. For Agda this is actually better anyway — you can typecheck all changed modules in one
compiler session rather than spinning up a new one per file.
    
The other thing to nail down is what each hook receives. Pre-extract needs both the source path (to know the Agda module) and the compiled HTML path
(to find and replace code blocks). Post-extract needs the node ID and its HTML. Post-render needs the output page path. File paths as arguments is
the simplest protocol — no custom serialisation, hooks can be shell scripts if needed.
