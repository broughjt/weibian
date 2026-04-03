I have both files fresh in context. Let me organize the corruptions by where in the pipeline they hit, and flag which ones currently panic vs. already produce diagnostics.

---

## HTML Structure Corruptions

These mutate `FileOutput.html`.

| # | Corruption | Current behavior | Diagnostic after fix |
|---|---|---|---|
| 1 | Remove the `wb-node` | diagnostic | "source file produced no wb-node" |
| 2 | Duplicate the `wb-node` in HTML (add second copy, add its id to spans) | diagnostic | "source file produced multiple wb-node elements" |
| 3 | Strip `identifier` from `wb-node` (leave entry in spans) | panic | "wb-node is missing an identifier" |
| 4 | Strip `identifier` from a `wb-subnode` (leave entry in spans) | panic | "wb-subnode is missing an identifier" |
| 5 | Add an identifier in HTML not present in spans | panic | needs new diagnostic |
| 6 | Duplicate a `wb-subnode` identifier in HTML | panic | needs new diagnostic |
| 7 | Strip `wb-title` from `wb-node` | diagnostic | "wb-node's first child must be a wb-title element" |
| 8 | Strip `wb-title` from a `wb-subnode` | diagnostic | "wb-subnode's first child must be a wb-title element" |
| 9 | Strip `transclude` from a `wb-subnode` | diagnostic | "wb-subnode is missing the transclude attribute" |
| 10 | Set `transclude` to invalid value on a `wb-subnode` | diagnostic | "wb-subnode has invalid transclude value: ..." |
| 11 | Strip `identifier` from a `wb-transclude` | diagnostic | "wb-transclude is missing an identifier" |
| 12 | Strip `counter` from a `wb-transclude` | diagnostic | "wb-transclude is missing a counter attribute" |
| 13 | Set invalid `counter` on a `wb-transclude` | diagnostic | "wb-transclude has invalid counter: ..." |
| 14 | Strip `data-counter` from a `wb:` link | diagnostic | "link is missing a data-counter attribute" |
| 15 | Set invalid `data-counter` on a `wb:` link | diagnostic | "link has invalid data-counter: ..." |

---

## Metadata Map Corruptions

These mutate the metadata maps without touching HTML. They represent metadata that the Typst compiler emitted for a node/transclusion/link that no longer exists in the HTML — the inverse of the HTML corruptions above.

| # | Corruption | Current behavior | Diagnostic after fix |
|---|---|---|---|
| 16 | Insert entry into `node_metadata` for an identifier absent from spans | panic | "unconsumed node metadata: ..." |
| 17 | Insert entry into `transclusion_metadata` for a counter with no matching `wb-transclude` | panic | "unconsumed transclusion metadata: ..." |
| 18 | Insert entry into `link_metadata` for a counter with no matching `wb:` link | panic | "unconsumed link metadata: ..." |

---

## Notes

**Corruptions 3, 4, 16–18 are deeply linked.** Stripping an identifier from a node (corruptions 3/4) leaves its `node_metadata` entry orphaned (corruption 16's effect). Similarly, stripping a transclusion counter (12/13) orphans its `transclusion_metadata` entry (17). This is the core of why `expected_errors` is non-trivial to compute — a single HTML corruption can cascade into metadata errors.

**Corruption 5** is the inverse of 3/4: the spans map claims a node exists that the HTML doesn't have an identifier for. This currently panics in `extract_node_content` at `spans.get(&identifier).expect(...)`. It doesn't have a diagnostic yet and needs one added.

**Corruption 6** (duplicate subnode id) currently panics at `assert!(displaced.is_none(), "bug: duplicate node identifier slipped past collect_node_spans")`. Also needs a new diagnostic.

**`collect_metadata` diagnostics are out of scope** for this corruption layer — they're only reachable through `TypstCompile`, not through `extract` directly, since `FileOutput` already carries the post-collection maps.
