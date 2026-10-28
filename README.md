# dramatis

Turn a collaborative MediaWiki site into a retrieval corpus for character agents: an offline
**forge** (Python) that harvests and packs the corpus, and a runtime **engine** (Rust) that
reads it.

Everything specific to one wiki (which pages to take, how to parse them, identity rules,
wording, language rules) lives in a **pack** that you supply. This repository ships none.

```
forge/    Python 3.13   sync · build · baseline · report · persona
engine/   Rust 2024     crates/folio · crates/index · crates/eval · crates/world
                        crates/agent · crates/api · crates/runtime
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

## Personas

```sh
./forge persona                 # generate and gate a persona per person; resumable
./forge persona --probe         # only a small probe set, with a readable file per subject
./forge persona --stale         # no network: list personas whose source material changed
./forge persona --stale --regenerate
./forge persona --only <name> --force
```

Generation uses the `chat` role of `$DRAMATIS_HOME/endpoints.toml` (`--endpoints` for another
file). Models billed per request run 3 calls at once by default (`--jobs`); on HTTP 429 every
worker waits together. A persona that fails a gate is not written. Reports go to
`<workspace>/artifacts/<pack>/persona/` (`REPORT.md`, `report.json`, `STALE.md`). Each reply is
cached under `persona/raw/`; rebuilding the folio drops the personas, and running
`./forge persona` again restores them from that cache with no model calls.

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

Talk to the persons of a corpus from a shell. The terminal and the desktop app are two front ends
of one back end: both build the same engine (`engine/crates/runtime`) and the terminal issues the
same `api` commands and renders the same events the app does, so the two cannot drift apart.

```sh
cd engine
cargo run -p dramatis-cli --release -- chat              # then /list, /talk <person>
cargo run -p dramatis-cli --release -- chat <person>     # open a direct channel at once
printf '/talk <person>\nhello\n/wrapup\n' | ./target/release/dramatis-cli chat
```

The corpus is `--folio`, else `$DRAMATIS_FOLIO`, else the `.folio` in `$DRAMATIS_HOME`
(default `~/.dramatis`). Endpoints come from `$DRAMATIS_HOME/endpoints.toml` and keys from the
system keychain exactly as in the app. A key is read once per process and only for a profile
that is used; with an ad-hoc signed build the first run of each new build triggers a macOS
keychain prompt (choose Always Allow). To stop the prompts coming back after every rebuild, sign
with a stable identity, see [Keychain prompts](#keychain-prompts).

The world (channels, memories, usage) is the app's: `$DRAMATIS_HOME/world.sqlite`, so the
terminal and the app share conversations, settings and endpoints, and may be open together
(the database is in WAL mode and writes wait for each other). Use `--home <dir>` for another
home directory, `--fresh` for a throwaway world (a temporary home that keeps your endpoints and
parameters and is deleted at exit), and `--user <name>` to set the user's name. On a terminal
the line is edited with arrow keys, Home/End and Up/Down history, and wide (CJK) characters count by their display width;
`--plain`, or piped input, reads plain lines with no prompt.

| Command | What it does |
| --- | --- |
| `/list [filter]` | persons with a persona, and whether the host is available |
| `/talk <person>` | open the direct channel; an id, display name, other form or alias |
| `/group <p1> <p2> ...` | start a group channel |
| `/host` | switch to the host's channel |
| `/go <n>` (`/open`) | after the host found people or made a group, a numbered list follows its reply; this switches to choice n |
| `/name <new name>` | set the name you are called by (`--user` sets it at start) |
| `/wrapup` | end the conversation and show the memories it wrote |
| `/memories [person]` | stored memories and who can see them, or those about or recalled by a person |
| `/history [n]` | reprint the last n messages (default 10) |
| `/search <query>` | corpus retrieval as the current speaker would see it; no model call |
| `/stats` | one row per model call of this session: shape, model, tokens, seconds (read from the meter, not part of the app's interface) |
| `/help`, `/quit` | Ctrl-D also leaves |

Anything else is sent to the current channel. Replies stream as they are written; scene lines
are dimmed, citation marks are hidden and the cited sources are listed after the reply, tool
actions get one dim line, a dim line says who is typing while a turn runs, and another gives
the call count and time. The prompt shows the channel and then your own name. Colour is off when
stdout is not a terminal or `NO_COLOR` is set. A failed call prints its error and the session
goes on. Ctrl-C while a reply is pending cancels the wait (the message you sent stays in the
history); at the prompt it clears the line, and Ctrl-D leaves.

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

## Keychain prompts

Profile keys live in the login keychain (service `dramatis`, account `profile.<name>`). macOS
lets an app read an item without asking only if the item's access list names that app. An
ad-hoc signed build (the default for `cargo build`) has an identity that changes with every
build, and the app and the terminal client are two different apps, so each new build asks again.
Signing both with one stable certificate makes the identity survive rebuilds.

One time, create a local code-signing certificate: Keychain Access, menu Certificate Assistant,
Create a Certificate; Identity Type Self Signed Root, Certificate Type Code Signing. The name is
yours to choose; the commands below use `dramatis` as an example. Check it is usable:

```sh
security find-identity -p codesigning        # lists the certificate by name
```

After every build, sign what you run. The identifier of the terminal client is fixed so it does
not carry a build hash. Replace `dramatis` with your certificate's name:

```sh
cd engine && cargo build --release -p dramatis-cli
codesign --force --sign dramatis --identifier dramatis.cli target/release/dramatis-cli
codesign -dv target/release/dramatis-cli 2>&1 | grep -E 'Identifier|Signature'
```

For the desktop app, build the bundle, clear extended attributes and sign it as a whole:

```sh
cd ui && pnpm build
cd ../engine/bins/dramatis && npx --prefix ../../../ui tauri build --bundles app
cd ../.. && xattr -cr target/release/bundle/macos/dramatis.app
codesign --force --deep --sign dramatis target/release/bundle/macos/dramatis.app
```

The first run of each signed program asks once more; choose Always Allow and it holds across
rebuilds. A key saved before signing keeps its old access list, so save it again (profile
settings, or delete and add it) if a prompt keeps coming back.
