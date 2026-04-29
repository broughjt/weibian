# Re-render Invalidation Tests (Stage 4, Reverse BFS)

These tests verify that the reverse BFS through the transclusion graph produces the correct re-render set. Run each test using `wb watch` against `~/scratch/BlogSource`, make the indicated edit, and compare the printed re-render set against the expected set.

## Transclusion graph summary (BlogSource)

```
index ──── tr ──→ 0005 ──── tr ──→ 0001
      \               \──── tr ──→ 0002
       \               \──── tr ──→ 0003
        \               \──── tr ──→ 0004
         \
          \─── tr ──→ 000S ─── tr ──→ 0006
          |               \─── tr ──→ 0007
          |                \─── ... (0008–000R, 22 poems total)
          |
          \─── ln ──→ 0000 ─── tr ──→ guo-IfTBenchmarkTypeNarrowing-2025
                           \── tr ──→ peng-StatisticalTypeInferenceIncompletePrograms-2023
```

`tr` = transclusion (appears in re-render propagation), `ln` = link (does not).

---

## Test 1 — Leaf poem

**Edit:** Make a trivial change to a poem, e.g. add a space in `@@0006--忏悔者的雕像张开手臂.typ` (node `0006`).

**Expected re-render set:** `{0006, 000S, index}`

**Rationale:** `0006` is a leaf node. `000S` (poems collection) transcludes it, so `000S`'s rendered body would change. `index` transcludes `000S`, so it is invalidated too. No other nodes transclude `0006` or `000S`.

**Result:** PASS — `{0006, 000S, index}`

---

## Test 2 — Mid-level collection node, no change to children

**Edit:** Make a trivial change to the prose in `@@0005--blog.typ` (node `0005`) without adding or removing any transclusions.

**Expected re-render set:** `{0005, index}`

**Rationale:** Only `0005` itself changed. Its children (`0001`–`0004`) are unchanged — editing a parent does not invalidate its children, only its ancestors. `index` transcludes `0005` and is therefore invalidated.

**Result:** PASS — `{0005, index}`

---

## Test 3 — Node that is linked to but not transcluded

**Edit:** Make a trivial change to `@@0000==CV--hanwen-guo.typ` (node `0000`).

**Expected re-render set:** `{0000}`

**Rationale:** `index` has a `ln()` (link) to `0000`, not a transclusion. Since links do not appear in the transclusion graph, the reverse BFS from `0000` finds no ancestors. `index` should NOT appear in the re-render set. This test distinguishes the transclusion graph from the links graph.

**Result:** PASS — `{0000}`

---

## Test 4 — Bibliography entry (deep in a non-index branch)

**Edit:** Make a trivial change to `@@guo-IfTBenchmarkTypeNarrowing-2025.typ` (node `guo-IfTBenchmarkTypeNarrowing-2025`).

**Expected re-render set:** `{guo-IfTBenchmarkTypeNarrowing-2025, 0000}`

**Rationale:** The bibliography entry is transcluded by `0000` (CV), so `0000` needs re-rendering. Nothing transcludes `0000` (index only links to it), so the propagation stops there. `index` should NOT appear.

**Result:** PASS — `{guo-IfTBenchmarkTypeNarrowing-2025, 0000}`

---

## Test 5 — Simultaneous edits in two independent branches

**Edit:** In the same debounce window, make trivial changes to both `@@0001--gödels-β-function.typ` (node `0001`) and `@@0006--忏悔者的雕像张开手臂.typ` (node `0006`).

**Expected re-render set:** `{0001, 0005, 0006, 000S, index}`

**Rationale:** Multi-source BFS combines both chains: `0001 → 0005 → index` and `0006 → 000S → index`. `index` is reachable from both paths but should appear only once. This tests that the BFS correctly deduplicates across multiple dirty nodes.

**Result:** PASS — `{0001, 0005, 0006, 000S, index}`
