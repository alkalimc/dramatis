# Engine: what exists, and what the corpus already decided

The engine is a **skeleton**: it reads a corpus, searches it, and measures itself. That is
the whole of it, and the ordering is deliberate — two freeze conditions (first-token
latency, resident memory) cannot be settled on paper, so the thing that answers them has to
run before the rest is designed around guesses.

## What exists

```
engine/
  crates/folio/    manifest validation · units · f16 vectors · roster · aliases · prompts
  crates/index/    lexical BM25 ∥ exact dense scan · RRF fusion · three-signal confidence
  bins/dramatis-cli/   inspect · search · bench
```

30 tests, of which 10 run against the real 58,853-unit corpus and skip when it is absent.

## What does not

The agent loop, world state, settlement, the approval gate, and the RPC layer. No empty
crate directories stand in for them: a tree of empty crates looks like progress and is not.

## Measured, so no longer open to design

| Constraint | Measured | Consequence |
| --- | --- | --- |
| 58,853 units, 120 MB of f16 vectors at 1024d | corpus build | An exact full scan is viable; no ANN, so the dimension-ablation curve stays clean |
| Lexical path p50 1.6 ms, p95 5.8 ms | `bench` | Comfortably inside budget. Was 13.2 ms before query-side stopword removal |
| Retrieval layer peak 68.5 MB | `bench` | The daemon's whole allowance is 350 MB, so there is room |
| Zero positional redundancy | corpus build | **No span-merge stage.** Units are stored once |
| `requires: [neighbor_expand]` | folio manifest | Widen a ranked hit to its neighbours, or serve truncated context. Refuse to load rather than degrade |
| Per-template size spread of 30× | corpus build | Scores need per-template calibration; `template_stats` ships for that |
| Segmenter recorded in the manifest | corpus build | Query-side segmentation reads it rather than assuming |

## Three things the skeleton found that reading code would not have

Worth stating, because they are the argument for building a measurement device early.

**A stopword cost 8× the latency budget.** `的` appears in 49,152 of 58,853 units, so
including it in a MATCH expression makes BM25 score nearly the whole corpus. Dropped from
queries, kept in the index — a phrase search may want it later.

**Statement preparation was the entire fetch cost.** 8.7 ms to fetch rows whose query
executes in 0.08 ms, because SQL was built per candidate count and so never cached. Fixed
shape, ordinals passed as one JSON parameter, `prepare_cached`.

**Filtering after truncation returns nothing.** Asking for two dialogue units from a corpus
holding 32,322 of them returned zero, because the filter ran on an already-cut list. Both
correctness and cost are fixed by pushing the filter into SQL.

A fourth is about measurement itself: `TASK_BASIC_INFO_64` returns success and a resident
size of zero on current macOS kernels, so the first benchmark reported a tidy 0.0 MB. A
system call that succeeds is not the same as a correct measurement, and two freeze
conditions rest on that number.

## Two contracts enforced by code, not discipline

* An unknown `format_version` refuses to load. A format bump means a column changed
  meaning, and guessing the new meaning is how a reader starts answering wrongly.
* A declared requirement this build does not implement refuses to load. Ignoring
  `neighbor_expand` would return answers that look complete and are not.

## Next

Encoder and reranker weights, which turn the dense path from written to measured. Then the
agent loop. The order is in the design repository's freeze document; the short version is
that anything gating a claim gets measured before anything built on that claim gets written.
