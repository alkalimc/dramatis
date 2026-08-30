# Engine: what exists and what does not

**Nothing is implemented yet.** This file records the intended shape so the forge can be
written against it, and so the first commit of Rust code has something to conform to.

Empty directories are deliberately absent: a tree of empty crates looks like progress and
is not. See `dramatis-design/architecture/engine.md` for the reasoning behind each piece,
and `dramatis-design/process/freeze.md` §4 for the order.

## Intended layout

```
engine/
  crates/
    folio    read a .folio: units · vectors · roster · aliases · prompts
    index    FTS5 lexical path ∥ exact SIMD dense path → RRF → rerank → confidence
    agent    the loop · OpenAI-compatible client · MCP client · approval gate
    world    world state · roster · bonds · tasks · settlement
    rpc      JSON-RPC 2.0, with JSON Schema exported for the client
  bins/
    dramatisd       the daemon
    dramatis-mcp    the retrieval layer as a standalone MCP server
    dramatis-cli    maintainer surface: index, evaluate, query interactively
```

## What the forge already fixes for it

The corpus is built, so several engine decisions are already constrained by measurement
rather than open:

| Constraint | Measured | Consequence for the engine |
| --- | --- | --- |
| 58,853 units, 120 MB of f16 vectors at 1024d | probe P2 | An exact full scan is viable; no ANN, so the dimension-ablation curve stays clean |
| Zero positional redundancy | probe P3 | **No span-merge stage.** Units are stored once |
| `requires: [neighbor_expand]` | folio manifest | The reader **must** widen a ranked hit to its neighbours, or long exchanges answer incompletely. Refuse to load rather than truncate silently |
| Per-template size spread of 30× | probe P2 | Scores need per-template calibration; `template_stats` is in the folio for exactly this |
| Segmenter identity recorded | probe P6 | Query-side segmentation must match the build side; read it from the manifest rather than assuming |

## First milestone

Skeleton plus retrieval, because it is what answers the two blocking probes:

```
load a .folio → hybrid search over 58,853 units → neighbour expansion → rerank
  acceptance: P11 (first-token latency) and P12 (resident memory) on target hardware
```

That milestone is itself a probe. Design freeze cannot complete without it — which is the
honest version of "freeze the design before writing code", rather than pretending the
memory and latency numbers can be settled on paper.
