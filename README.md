# dramatis

Turn a collaborative MediaWiki site into a retrieval corpus for character agents: an offline
**forge** (Python) that harvests and packs the corpus, and a runtime **engine** (Rust) that
reads it.

Everything specific to one wiki (which pages to take, how to parse them, identity rules,
wording, language rules) lives in a **pack** that you supply. This repository ships none.

```
forge/    Python 3.13   sync · build · baseline · report
engine/   Rust 2024     crates/folio · crates/index · crates/eval · bins/dramatis-cli
```

## Install

```sh
cd forge
make install                               # venv + editable install (jieba, tokenizers)
export DRAMATIS_PACKS=/path/to/rules       # directory that contains packs/<name>/
export DRAMATIS_PACK=<name>                # or pass --pack <name> to every command
./forge --help
make test lint                             # pytest + ruff on a synthetic toy pack
```

Artifacts are written outside the repository, to `<workspace>/artifacts/<pack>/` (override
with `DRAMATIS_FORGE_HOME` or `--home`).

## Two commands

```sh
./forge sync      # network: bring the archive level with the site, then build
./forge build     # offline: normalize → samples → folio → evals → reports
```

`sync` is the only command that makes requests. The first run enumerates every seed set and
fetches every page; later runs read the site's change feed from the stored watermark,
re-fetch what changed, fetch what entered scope and drop what left it. It is resumable
(held pages are never fetched twice) and rate-limited. `--full` re-fetches everything,
`--dry-run` shows the plan and stops, `--no-rescope` skips re-enumeration for a quick
increment, `--rate` caps requests per second.

`build` is deterministic and needs no network: re-run it after changing a pack rule.

## Review every sync

Each build rewrites `samples/INDEX.md`: sync status (watermark, pages held, record counts and
their change, guard tally), a scan for markup left in record text, **every page the last
sync changed, added or removed**, and fixed boundary cases per route (first page, largest,
smallest, most records, first with findings, first that produced nothing). Each page is
written three ways: as fetched, as records, as archived text. Read it, then:

```sh
./forge baseline accept    # record the reviewed counts; guards compare later runs to them
./forge baseline show      # compare without accepting
```

## Reports

All generated; do not edit them.

| Command | Writes |
| --- | --- |
| `./forge report samples` | `samples/`, alone |
| `./forge report figures [--show]` | `reports/FIGURES.md`: measured values, pass/fail against pack targets |
| `./forge report figures --check <dir>` | audits Markdown in `<dir>`: dangling links, undefined ids, unknown figure keys |
| `./forge report coverage` | `reports/COVERAGE.md`: what was taken from the source and why the rest was not |
| `./forge report attribution [--editors]` | `reports/ATTRIBUTION.md`, `reports/NOTICE.md`: per-page provenance |
| `./forge report inspect <title>` | one page at every stage |

## Query a corpus

```sh
cd engine && cargo build --release
export DRAMATIS_FOLIO=/path/to/artifacts/<pack>/<pack>.folio
./target/release/dramatis-cli inspect
./target/release/dramatis-cli search "<query>" --top-k 6 --expand 1
./target/release/dramatis-cli evaluate --suite ../artifacts/<pack>/evals/structural.queries.jsonl \
    --markdown ../artifacts/<pack>/reports/RESULTS.md
./target/release/dramatis-cli bench
```

## Writing a pack

The contract is `forge/src/dramatis_forge/pack.py`: a module `packs/<name>/__init__.py`
that defines `PACK = Pack(...)` with seed sets, routes to page parsers, inline markup rules,
identity rules, chunking policy, figures, text and stopwords. Missing text keys fall back to
the English defaults in `forge/src/dramatis_forge/text.py`.

## Licence

Code: Apache-2.0. A corpus built with this tool is not covered by that licence; it carries
whatever terms its source has, and the attribution report gives the per-page provenance
needed to honour them.
