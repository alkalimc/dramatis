# dramatis

Turn a collaborative MediaWiki site into a retrieval corpus for character agents: an offline
**forge** (Python) that harvests and packs the corpus, and a runtime **engine** (Rust) that
reads it.

Everything specific to one wiki (which pages to take, how to parse them, identity rules,
wording, language rules) lives in a **pack** that you supply. This repository ships none.

```
forge/    Python 3.13   sync · build · baseline · report · persona
engine/   Rust 2024     crates/folio · crates/index · crates/eval · crates/world
                        crates/agent · crates/api
                        bins/dramatis-cli · bins/dramatis (desktop app, Tauri 2)
ui/       TypeScript    React 19 + Vite, talks only to the engine's `api` commands
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

## Terminal

Talk to the persons of a corpus from a shell, with the same agent the desktop app uses.

```sh
cd engine
cargo run -p dramatis-cli --release -- chat              # then /list, /talk <person>
cargo run -p dramatis-cli --release -- chat <person>     # open a direct channel at once
printf '/talk <person>\nhello\n/wrapup\n' | ./target/release/dramatis-cli chat
```

The corpus is `--folio`, else `$DRAMATIS_FOLIO`, else the `.folio` in `$DRAMATIS_HOME`
(default `~/.dramatis`). Endpoints come from `--endpoints`, else
`$DRAMATIS_HOME/endpoints.toml`, and keys from the system keychain exactly as in the app. The
first run of each new build triggers a macOS keychain prompt: choose Always Allow.

The world (channels, memories, usage) lives in `$DRAMATIS_HOME/cli/world.sqlite`, separate
from the app's. Use `--world <path>` for another file (the app's, for example), `--fresh` for
a throwaway in-memory world, and `--user <name>` to set the user's name.

| Command | What it does |
| --- | --- |
| `/list [filter]` | persons with a persona, and whether the host is available |
| `/talk <person>` | open the direct channel; an id, display name, other form or alias |
| `/group <p1> <p2> ...` | start a group channel |
| `/host` | switch to the host's channel |
| `/wrapup` | end the conversation and show the memories it wrote |
| `/memories [person]` | stored memories and who can see them, or those a person may recall |
| `/history [n]` | reprint the last n messages (default 10) |
| `/search <query>` | retrieval only, as the current speaker would see it; no model call |
| `/stats` | one row per model call of this session: shape, model, tokens, seconds |
| `/help`, `/quit` | Ctrl-D also leaves |

Anything else is sent to the current channel. Replies stream as they are written; scene lines
are dimmed, citation marks are hidden and the cited sources are listed after the reply, tool
actions get one dim line, and a dim line gives the call count and time. Colour is off when
stdout is not a terminal or `NO_COLOR` is set. A failed call prints its error and the session
goes on. Ctrl-C while a reply is pending cancels the wait (the message you sent stays in the
history); at the prompt it leaves.

## Writing a pack

The contract is `forge/src/dramatis_forge/pack.py`: a module `packs/<name>/__init__.py`
that defines `PACK = Pack(...)` with seed sets, routes to page parsers, inline markup rules,
identity rules, chunking policy, figures, text and stopwords. Missing text keys fall back to
the English defaults in `forge/src/dramatis_forge/text.py`.

## Licence

Code: Apache-2.0. A corpus built with this tool is not covered by that licence; it carries
whatever terms its source has, and the attribution report gives the per-page provenance
needed to honour them.

## Desktop app

macOS 13+ on Apple Silicon. Node 24+ and pnpm (the version in `ui/package.json`).

```sh
cd ui && pnpm i                              # once
pnpm dev                                     # terminal 1: Vite on 127.0.0.1:5173
cd engine && cargo run -p dramatis           # terminal 2: debug app, loads the dev server
```

A debug build loads the UI from the dev server; nothing needs to exist in `ui/dist`. A build
that embeds the UI (`--features custom-protocol`, which `pnpm tauri build` enables) needs
`pnpm build` in `ui/` first.

The browser preview (`pnpm dev` without the app) shows the neutral English text. To preview a
corpus's own wording, export it once and reload; the file is git-ignored:

```sh
cd forge && DRAMATIS_PACKS=<dir holding packs/> ./forge report wording ../ui/wording.local.json --pack <name>
cd ui && pnpm dev                            # the mock picks up ui/wording.local.json
```

`ui/src/api.gen.ts` is generated from `engine/crates/api`; never edit it. After changing
a command or event type:

```sh
cd engine && cargo test -p dramatis --test bindings   # rewrites ui/src/api.gen.ts
```

The model-facing tool schemas are snapshotted in `engine/crates/api/tests/tools.snapshot.json`;
after an intended change run `UPDATE_SNAPSHOTS=1 cargo test -p api --test tools`.

UI checks: `cd ui && pnpm exec tsc --noEmit && pnpm exec vitest run`.
