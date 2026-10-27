# research

Fine-tuning and evaluation of the two retrieval models (embedding and reranker) on training
pairs that the wiki's structure gives for free: redirects, disambiguation pages, section
hierarchy, speaker attribution and page-to-person identity. An LLM-written synthetic query
set is the control. This project only consumes an export; it never calls an LLM endpoint
and never changes a product default. A fine-tuned model that wins becomes an option, not
the default.

It is its own project with its own dependencies (torch, sentence-transformers). Keep it out
of the forge environment.

## Setup

```sh
cd research
make install        # uv venv --python 3.13 && uv pip install -e '.[lora]' --group dev
make test lint
```

Versions are pinned exactly in `pyproject.toml`. `peft` (low-rank adapters, `--lora-rank`) is
an optional extra; a full fine-tune of a 0.6B model needs roughly 10 GB for weights, gradients
and optimizer state, so on a 16 GB machine use `--lora-rank 8 --max-seq-length 256` and a small
`--batch-size`. Training runs on CUDA, then Apple MPS, then CPU (`DRAMATIS_RESEARCH_DEVICE`
or `--device` override).

If an editable install is invisible on your machine (hidden `.pth` files), run with
`PYTHONPATH=src`; the Makefile and the test configuration already do.

## Models

| role | default | licence |
| --- | --- | --- |
| embedding | `Qwen/Qwen3-Embedding-0.6B` | Apache-2.0 |
| reranker | `Qwen/Qwen3-Reranker-0.6B` | Apache-2.0 |

Both ids are flags (`--base`). The embedding model uses last-token pooling and an
instruction in front of queries (`Instruct: <task>\nQuery:<text>`, documents carry none);
`--query-instruction` sets the task text and the value is stored in `run.json` so evaluation
formats queries the way training did. The reranker is a causal language model, not a classic
cross-encoder: a pair is rendered into a fixed prompt and scored by the next-token logits of
"yes" against "no" (`encoders.YesNoReranker`). It is trained with a listwise loss: the
positive must outscore its negatives, using the same code path as scoring.

The scripts never download weights by themselves in tests (tests build tiny random models).
Real runs read the Hugging Face cache, or download on first use unless `HF_HUB_OFFLINE=1`.

## Input

A research root directory (`--root`, or `DRAMATIS_RESEARCH_ROOT`) holding the export:

| file | rows |
| --- | --- |
| `units.jsonl` | `id, header, text, template, page, persons` |
| `pairs.train.jsonl`, `pairs.test.jsonl` | `query, positive, negatives (unit ids), family, split, page, persons` |
| `pairs.synth.train.jsonl`, `pairs.synth.test.jsonl` | same shape; negatives usually empty |
| `eval.structural.test.jsonl` | engine evaluation-suite rows (`qid, family, text, stratum, gold, hard_negatives`) |
| `eval.verify.jsonl` | optional; engine-suite rows or pair rows |
| `split.json` | optional; `persons.test` and `pages.test` lists declared by the exporter |
| `cost.json` | optional; `{"synthetic_llm_calls": N}` for the cost column |

Missing sets are skipped and reported as absent, never invented.

## No-leak guarantee

Every trainer gets its data from `data.select_training_pairs`, which raises `LeakError`
(training does not start) when a training pair, its positive or any of its negatives:

- is on a page that a held-out pair names, or on a page `split.json` declares as test;
- involves a held-out person: `split.json` `persons.test` when present, otherwise every
  person named by a held-out pair (strict);
- is itself a held-out unit (a gold or positive unit of any held-out set); or
- carries a split label other than `train`, or names a unit that does not exist.

The check runs on the full candidate list before `--limit`, so a truncated run cannot hide a
leak a full run would hit. Random negatives are drawn only from units outside the held-out
sets. `run.json` records which source defined the held-out persons. Units are held out
individually rather than by their page, because a shared index page can hold both training and
held-out units.

## Commands

```sh
export DRAMATIS_RESEARCH_ROOT=/path/to/research     # the export directory
alias dr='PYTHONPATH=src .venv/bin/python -m dramatis_research'

# fine-tuning (research 3)
dr train-embed  --pairs structural|synthetic|both --negatives structural|random --limit N --seed 0
dr train-rerank --pairs structural --negatives structural --hard-negatives 3

# fixed-window units, derived from units.jsonl (see below)
dr windows --size 400

# evaluation (research 5)
dr evaluate \
  --system ft-structural=<embed-run-id>,<rerank-run-id> \
  --system ft-synthetic=<embed-run-id> \
  [--engine-cli target/release/dramatis-cli --folio corpus.folio]

# the four ablations
dr ablate plan
dr ablate run --epochs 1 --lora-rank 8
```

Flags shared by the trainers: `--pairs`, `--negatives`, `--limit N` (data-size ablation; the
order is a seeded shuffle, so a smaller limit is a subset of a larger one), `--seed`,
`--drop-family F` (repeatable, per-source ablation), `--window N`, `--hard-negatives K`,
`--epochs`, `--batch-size`, `--learning-rate`, `--max-seq-length`, `--lora-rank`, `--bf16`,
`--gradient-checkpointing`, `--run-id`.

### Outputs

Runs go to `<root>/runs/<run-id>/` (`--runs-dir` overrides): `model/` and `run.json`. The
`run.json` holds the config, SHA-256 of every input file, a digest of the exact selected
pairs, the count per family, timing, device and versions, `train.gpu_hours` (accelerator wall
time; CPU time is `train.cpu_hours`) and `llm_calls_for_data` (0 for structural pairs).
`evaluate` writes `RESULTS-research.md` and `RESULTS-research.json` next to the runs.

### Evaluation

Systems: BM25, base embedding, base embedding plus base rerank, and any fine-tuned
combination given with `--system NAME=EMBED[,RERANK]` (each part is `base`, `none` or a run
id). All systems rank the same pool of units, so rows within a set are comparable.

- The structural, synthetic and verify sets are always reported separately. There is no
  combined score; the only average is a macro average over families inside one set.
- Per family: nDCG@10, recall@10, recall@50, MRR@10, negative win rate (a hard negative
  outranks every relevant unit; shown as n/a for families without negatives). The metric
  definitions match the engine's evaluation crate.
- Families that BM25 alone solves (nDCG@10 at or above `--lexical-floor`, default 0.8) are
  marked `*`: a sanity floor, not evidence of retrieval quality.
- The engine's own lexical result can be added as a reference row on the structural set: give
  `--engine-json` (output of `dramatis-cli evaluate --json`) or let `--engine-cli` and
  `--folio` run it. It searches the full index rather than the exported pool, so it is a
  cross-check, not a peer row.
- Cost columns: LLM calls spent on the training data and accelerator hours of the training
  runs behind the system.
- `--rerank-depth` (default 20) is how many embedding hits the reranker reorders; the rest
  keep the embedding order. `--max-queries` caps each set with every n-th query per family.

### Ablations

`dr ablate plan` lists the runs; `dr ablate run` trains whatever is missing (existing runs are
reused) and writes the reports.

| ablation | runs |
| --- | --- |
| drop one supervision source | structural pairs minus one family, once per family |
| structural vs random negatives | `--negatives random` for structural and for both |
| data size | `--limit` over `--sizes` (default 1000,3000,10000,30000) |
| typed units vs fixed windows | a model trained on windows, scored on the window pool |

The main systems (structural, synthetic, both) are part of the plan when synthetic pairs
exist. The window comparison goes to `RESULTS-research-windows.md`.

### Fixed windows

`dr windows --size N` (and `--window N` on the trainers and `evaluate`) derives a unit file
without any typed structure: units are grouped by page in file order, their texts joined with
a newline, and the page text is cut into consecutive windows of N characters. Headers and
templates are dropped; a window keeps its page and the union of the persons of the units it
overlaps, so the leak rules apply unchanged. To replay the same pairs and queries, a typed
unit id maps to the windows that overlap it (best overlap as the positive; relevant windows
are all those overlapping a relevant unit; negatives exclude any window overlapping the
positive). Queries whose gold maps to nothing are dropped and counted in the report metadata.
Window scores are on a different pool than typed scores, so compare them only in the
windows report.

## Tests

`make test` runs on CPU in about fifteen seconds with tiny randomly initialised models and
offline stand-ins (a hashing embedder and a word-overlap reranker): data loading and the
split guard, metrics against hand-computed values, BM25, windows, the ablation flags, the
embedding and reranker trainers (loss decreases, checkpoints reload) and the report.
