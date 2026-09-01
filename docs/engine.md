# Engine: what exists, and what measurement already settled

The engine is a **skeleton**: it reads a corpus, searches it, and measures itself. That is
the whole of it, and the ordering is deliberate — two of the project's freeze conditions
(first-token latency, resident memory) cannot be settled on paper, so the thing that
answers them has to run before the rest is designed around guesses.

## What exists

```
engine/
  crates/folio/    manifest validation · units · f16 vectors · roster · aliases · prompts
  crates/index/    lexical BM25 ∥ exact dense scan · RRF fusion · three-signal confidence
  crates/eval/     suite runner and retrieval metrics
  bins/dramatis-cli/   inspect · search · bench
```

Ten of its tests run against a real corpus and skip when none is present. This repository
ships no corpus and no pack: it is mechanism, and the rules that drive it are supplied
from outside.

## What does not

The agent loop, world state, settlement, the approval gate, and the client. No empty crate
directories stand in for them: a tree of empty crates looks like progress and is not.

## Measured, so no longer open to design

Figures live with the artifact that produced them, not here — a number copied into prose
drifts from the build it came from, and this file cannot be re-derived from a corpus. Run
`dramatis-forge report figures` against a built corpus for current values.

| Constraint | How it was settled | Consequence |
| --- | --- | --- |
| Unit count and vector footprint | corpus build | An exact full scan is viable at this scale; no ANN, so a dimension-ablation curve stays free of approximation error |
| Lexical path latency | `bench` | Comfortably inside budget, after query-side stopword removal |
| Retrieval layer peak resident memory | `bench` | Leaves room inside the runtime's whole allowance |
| Zero positional redundancy | corpus build | **No span-merge stage.** Units are stored once |
| `requires: [neighbor_expand]` | folio manifest | Widen a ranked hit to its neighbours, or serve truncated context. Refuse to load rather than degrade |
| Order-of-magnitude size spread across unit types | corpus build | Scores need per-type calibration; `template_stats` ships for that |
| Segmenter recorded in the manifest | corpus build | Query-side segmentation reads it rather than assuming |

## Three things the skeleton found that reading code would not have

Worth stating, because they are the argument for building a measurement device early.

**A stopword cost most of the latency budget.** A single high-frequency function word
appeared in the large majority of units, so including it in a MATCH expression made BM25
score nearly the whole corpus. Dropped from queries, kept in the index — a phrase search
may want it later. Which words qualify is a property of the language and the corpus, so
the list belongs to a pack, not here; the mechanism is a query-side filter.

**Statement preparation was the entire fetch cost.** Milliseconds to fetch rows whose
query executes in a fraction of one, because SQL was built per candidate count and so
never cached. Fixed shape, ordinals passed as one JSON parameter, `prepare_cached`.

**Filtering after truncation returns nothing.** Asking for a couple of units of one type
from a corpus holding tens of thousands of them returned zero, because the filter ran on
an already-cut list. Both correctness and cost are fixed by pushing the filter into SQL.

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
agent loop. The short version of the ordering: anything gating a claim gets measured before
anything built on that claim gets written.
