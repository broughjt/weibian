**Priority 1**

1. Fix config safety around output deletion.
   Files: [src/build.rs](/home/jackson/repositories/weibian/src/build.rs), [src/watch.rs](/home/jackson/repositories/weibian/src/watch.rs), [src/config.rs](/home/jackson/repositories/weibian/src/config.rs)
   Add validation in `BuildConfig::try_load` before either code path can call `remove_dir_all`.
   Reject obviously dangerous targets: `/`, home, project root, `input_directory`, and anything that is an ancestor of `input_directory`.
   Decide explicitly whether `output_directory` may live inside `input_directory`; if not, reject it. If yes, enforce exclusion consistently.

2. Normalize `output_directory` relative to config root.
   File: [src/config.rs](/home/jackson/repositories/weibian/src/config.rs)
   Make it follow the same rule as `input_directory`, `public_directory`, and templates.
   This is a correctness fix, not a style fix.

3. Fix index-node URL generation.
   File: [src/config.rs](/home/jackson/repositories/weibian/src/config.rs)
   Make `RenderConfig::href` and `BuildConfig::output_path` agree on the index-node case when `trailing_slash = true`.
   Add a small unit test for `href`/`output_path` combinations.

**Priority 2**
1. Restore a real test suite.
   Files: [src/compiler/tests.rs](/home/jackson/repositories/weibian/src/compiler/tests.rs), [src/compiler/tests/mock.rs](/home/jackson/repositories/weibian/src/compiler/tests/mock.rs), [src/compiler/tests/stateless.rs](/home/jackson/repositories/weibian/src/compiler/tests/stateless.rs)
   The best plan is to get the stateless reference model back online and test incremental `Compiler` behavior against it.
   That gives high leverage because `process()` is stateful and graph-heavy.

2. Make quality gates actually pass.
   File: [src/compiler/tests/mock.rs](/home/jackson/repositories/weibian/src/compiler/tests/mock.rs)
   Either re-enable the tests that use this module, or mark the scaffolding with targeted `#[allow(dead_code)]` as a temporary measure.
   `cargo clippy --all-targets -- --deny warnings` should be green all the time.

3. Add focused regression tests for the risky areas.
   Files: [src/compiler.rs](/home/jackson/repositories/weibian/src/compiler.rs), [src/compiler/extract.rs](/home/jackson/repositories/weibian/src/compiler/extract.rs)
   Cover:
   - node rename across files
   - transclusion cycle creation/removal
   - metadata-only updates
   - dangling links/transclusions becoming resolved
   - removal of a transcluded node
   - index-node path generation

**Priority 3**
1. Split `Compiler::process` into named passes.
   File: [src/compiler.rs](/home/jackson/repositories/weibian/src/compiler.rs)
   I would extract at least:
   - `collect_graph_diagnostics`
   - `analyze_affected_nodes`
   - `compute_backmatter_updates`
   - `render_affected_nodes`
   - `build_output_plan`
   The current logic is thoughtful, but too much depends on one long function staying mentally loaded.

2. Introduce small structs for transient state.
   File: [src/compiler.rs](/home/jackson/repositories/weibian/src/compiler.rs)
   Instead of many local `HashSet`s and `HashMap`s, wrap them in something like `ProcessInputs` and `ProcessState`.
   That makes invariants easier to document and test.

3. Move invariants into a dedicated debug helper.
   File: [src/compiler.rs](/home/jackson/repositories/weibian/src/compiler.rs)
   Replace ad hoc `assert!`s at the top and throughout with a `debug_assert_invariants()` method.
   That makes the safety model visible and centralized.

**Priority 4**
1. Deduplicate `build.rs` and `watch.rs`.
   Files: [src/build.rs](/home/jackson/repositories/weibian/src/build.rs), [src/watch.rs](/home/jackson/repositories/weibian/src/watch.rs)
   They repeat setup, file discovery, world construction, and diagnostics emission.
   A shared helper would reduce drift and make future fixes cheaper.

2. Tighten diagnostic fidelity.
   Files: [src/compiler.rs](/home/jackson/repositories/weibian/src/compiler.rs), [src/compiler/extract.rs](/home/jackson/repositories/weibian/src/compiler/extract.rs)
   The TODO about detached spans is worth doing once tests exist.
   Better spans will matter a lot for a note compiler.

3. Clean up minor rough edges.
   Files: [src/file_store.rs](/home/jackson/repositories/weibian/src/file_store.rs), [src/watch.rs](/home/jackson/repositories/weibian/src/watch.rs)
   Remove stale TODOs, decide on the commented API surface, and clarify a few naming inconsistencies.

If I were doing this as actual work, I’d sequence it as:
1. config safety/path fixes
2. re-enable tests and clippy
3. refactor `Compiler::process`
4. deduplicate build/watch

If you want, I can take the first pass at Priority 1 and 2 directly in the codebase.
