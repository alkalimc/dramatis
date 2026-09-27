# dramatis

Turn a collaborative MediaWiki site into a retrieval corpus for character agents: an offline
**forge** (Python) that harvests and packs the corpus, and a runtime **engine** (Rust) that
reads it.

Everything specific to one wiki — which pages to take, how to parse them, identity rules,
wording, language rules — lives in a **pack** that you supply. This repository ships none.

```
forge/    Python 3.13   harvest · normalize · corpus · evals · report · baseline
engine/   Rust 2024     crates/folio · crates/index · crates/eval · bins/dramatis-cli
```

## Install

```sh
cd forge
make install                               # venv + editable install (jieba, tokenizers)
export DRAMATIS_PACKS=/path/to/rules       # directory that contains packs/<name>/
export DRAMATIS_PACK=<name>                # or pass --pack <name> to every command
./forge --help
```

Artifacts are written outside the repository, to `<workspace>/artifacts/<pack>/`
(override with `DRAMATIS_FORGE_HOME`).

## Build a corpus

```sh
./forge harvest sync        # first time: enumerate, fetch, normalize (network, resumable)
./forge harvest update      # afterwards: incremental, from the site's change feed
./forge baseline accept     # record reviewed counts; guards compare later runs to them
./forge corpus build        # records → <pack>.folio
./forge evals build         # folio → structural benchmark suite
```

Every `harvest sync` and `harvest update` ends by writing a reading sample to
`artifacts/<pack>/samples/INDEX.md`: sync status, record counts and their change, guard
findings, markup left in any record, and a few pages per route shown as source → records →
archived text. Review it after each sync. `--no-samples` skips it; `./forge report samples`
regenerates it.

## Reports

| Command | Writes |
| --- | --- |
| `./forge report figures` | `FIGURES.md`: measured values, with pass/fail against pack targets |
| `./forge report coverage` | `COVERAGE.md`: what was taken from the source and why the rest was not |
| `./forge report attribution` | `ATTRIBUTION.md`, `NOTICE.md`: per-page provenance and terms |
| `./forge report inspect <title>` | one page at every stage |
| `./forge report figures --check <dir>` | audits Markdown in `<dir>` for stale figures and broken references |

## Query a corpus

```sh
cd engine && cargo build --release
export DRAMATIS_FOLIO=/path/to/artifacts/<pack>/<pack>.folio
./target/release/dramatis-cli inspect
./target/release/dramatis-cli search "<query>" --top-k 6 --expand 1
./target/release/dramatis-cli evaluate --suite <suite.jsonl> --markdown RESULTS.md
./target/release/dramatis-cli bench
```

## Writing a pack

The contract is `forge/src/dramatis_forge/pack.py`: a module `packs/<name>/__init__.py` that
defines `PACK = Pack(...)` — seed sets, routes to page parsers, inline markup rules, identity
rules, chunking policy, figures, text, stopwords. Missing text keys fall back to the English
defaults in `forge/src/dramatis_forge/text.py`.

## Licence

Code: Apache-2.0. A corpus built with this tool is not covered by that licence; it carries
whatever terms its source has, and `report attribution` produces the per-page provenance
needed to honour them.
