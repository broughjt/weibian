What Shake gives you

Shake's core value is a correct dependency DAG with incremental rebuilds. You declare "to produce file X I need files A and B", and Shake figures out
ordering, parallelism, and staleness automatically. Critically, Shake supports dynamic dependencies — a rule can need additional files it only
discovers at runtime (e.g. after parsing a source file to find its transclusion targets). 1lab uses exactly this architecture, and the watch mode is
an add-on via fsnotify.

Does it fit Weibian?
    
There's a fundamental tension. Weibian's transclusion resolution is a whole-graph operation — to render NodeA.html you need the compiled content of
every node it transcludes, which requires knowing the full node store. This is why Weibian keeps everything in memory across a build. In a Shake
model you'd express transclusion as dynamic need calls:

"dist/*.html" %> \out -> do
raw <- readRawHtml out
need ["dist" </> dep <.> "html" | dep <- extractTransclusions raw]
resolveAndWrite out

But Shake would then invoke Weibian per-file, losing the in-memory graph. You'd either have to split Weibian into per-file sub-commands (expensive,
cold-start each time) or treat Weibian as a black box that processes everything in one batch — at which point Shake can't see inside it anyway.
    
The pragmatic split
    
The practical approach is: Shake orchestrates around Weibian, not inside it.
    
src/**/*.lagda.typ
↓  (Shake: runs wb compile — whole batch, all transclusions resolved)
_build/wb/**/*.html         (Weibian's output)
↓  (Shake: per-file agda-highlighter rule)
dist/**/*.html + dist/types/*.json    (final output)
    
src/frontend/**/*.ts
↓  (Shake: esbuild rule)
dist/bundle.js
    
Weibian is a black box that Shake calls as one step. Post-processing rules depend on Weibian's outputs. Shake handles the ordering (Agda highlighting
can't run until Weibian finishes the affected file), incremental staleness, and parallelism across independent files.
    
Can you accomplish both use cases?
    
Agda post-processing: Yes, cleanly. This is exactly what Shake is good at — the dependency chain source → wb output → agda-highlighted output is a
textbook Shake DAG. When a .lagda.typ file changes, Shake rebuilds Weibian's output for that file, detects the downstream dist/Module.html is stale,
runs the highlighter. The ordering problem from before is solved for free.
    
Graph view: Yes. A Shake rule depending on all of Weibian's HTML outputs can run a tool to produce contentIndex.json. Though honestly this is simple
enough that Weibian could just emit it itself.
    
Is it better than a hooks system in Weibian?
    
For your personal setup, yes — Shake is a better orchestrator than anything Weibian could bolt on. It's principled, handles complex dependency graphs
correctly, and you don't have to design or maintain the hook machinery.
    
For Weibian as a general tool others would use, it's a significant barrier. Most people taking notes in Typst don't want to write a Shakefile in
Haskell. Weibian would still need good standalone defaults.
    
The conclusion this points toward: keep Weibian self-contained for the common case, and make it a good citizen for power users who want to wrap it in
Shake. Concretely, that means:
    
- Weibian does not need a hooks system
    - Weibian's watch mode is useful for the simple case (just notes, no Agda)
- For the Agda+graph use case, a Shakefile is the right tool and Weibian runs as one step within it
    - The main thing Weibian needs to support this is a clean way to run as a batch command targeting specific files (so Shake can invoke it for just the
changed subset rather than always rebuilding everything)

I agree with basically all of your points. Weibian definitely shouldn't fold itself inside out so that Shake can do its dependency tracking for
it. I also agree about the batch command. Shake should be able to call Weibian as one part of a larger build process, and if file watching and
hot reloading is going to work for this larger build process, the batch Weibian build command probably needs to not rebuild everything from
scratch everytime.
    
The problem with only rebuilding specific files is that if A changes and B, C, D transclude A or A affects B, C, or D's backmatter, then we need
to rerender B, C, and D too. Do you see the problem?
    
Is the solution to cache `Compiler`'s build state on the disk? Then `weibian build` could use this to know that it doesn't need to rebuild
everything, just the things which are affected by A.
    
    ● Yes, you've identified the exact problem. Shake sees A.lagda.typ changed and calls wb build, but Weibian's transclusion and backmatter graphs are
invisible to Shake — so Shake can't know that B, C, D also need rebuilding. If Weibian only rebuilds A, the outputs for B, C, D are stale.
    
And yes, a disk-persisted build state is the right solution. Here's why it works cleanly:
    
What gets persisted:
- The import graph (which .typ files import which)
    - The transclusion/backmatter graph (which nodes depend on which)
- The compiled node data (raw HTML, titles, metadata for each node)
    - File hashes for all source files
    
When Shake calls wb build:
1. Weibian loads the cache from .weibian-cache/
2. Scans source files and detects which changed by hash
3. Runs reverse-BFS on both graphs to find the full affected set {A, B, C, D}
4. Recompiles and re-renders only those nodes
5. Writes dist/A.html … dist/D.html
6. Saves the updated cache
   
Shake then sees dist/B.html, dist/C.html, dist/D.html changed (via its own file digest tracking) and triggers downstream rules — Agda highlighting,
whatever — for exactly those files. The cross-tool coordination works correctly without Shake ever needing to understand Weibian's internal
dependency model.
    
The important observation: this is exactly what the watch loop already does, just in memory. The Watcher struct holds the import graph, compiler
state, and file store across iterations and uses reverse-BFS to find the affected set on each change. Persisting that state to disk makes the same
logic available to a one-shot wb build command. wb watch could even just be wb build in a loop driven by notify, reusing the same code path.
    
The one thing to be careful about: cache invalidation on config changes. If weibian.toml changes (different templates, different include/exclude
patterns), the cache needs to be fully invalidated and rebuilt from scratch.
