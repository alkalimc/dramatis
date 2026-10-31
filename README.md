# dramatis

Turn a collaborative MediaWiki site into a retrieval corpus for character agents: an offline
**forge** (Python) that harvests and packs the corpus, and a runtime **engine** (Rust) that
reads it.

Everything specific to one wiki (which pages to take, how to parse them, identity rules,
wording, language rules) lives in a **pack** that you supply. This repository ships none.

```
forge/    Python 3.13   sync · build · wording · baseline · report · persona
engine/   Rust 2024     crates/folio · crates/index · crates/eval · crates/world
                        crates/llm · crates/agent · crates/api · crates/runtime
                        bins/dramatis-cli · bins/dramatis (desktop app, Tauri 2)
ui/       TypeScript    React 19 + Vite, talks only to the engine's `api` commands
```

## Requirements

- Rust 1.85 or newer (`engine/`).
- Node 24.15 or newer and pnpm 12.3.4, the version in `ui/package.json` (`ui/`, the desktop app only).
- Python 3.13 with [uv](https://docs.astral.sh/uv/) (`forge/`). `make install` installs the `.[zh,tokens]` extras; `.[zh]` adds jieba word segmentation for CJK text.
- Perl and make, to build the vendored OpenSSL under the SQLCipher dependency.
- A pack and a built `.folio` corpus. This repository ships neither; see [Writing a pack](#writing-a-pack).
- The desktop app and the signing steps below are macOS-only for now. The forge and the terminal client do not depend on the platform.

## Install

```sh
cd forge
make install                               # venv + editable install (jieba, tokenizers)
export DRAMATIS_PACKS=/path/to/rules       # directory that contains packs/<name>/
export DRAMATIS_PACK=<name>                # or pass --pack <name> to every command
./forge --help
make test lint                             # pytest + ruff on a synthetic toy pack
```

Artifacts are written outside this repository, to `dramatis-artifacts/<pack>/` next to the
repository folder, or inside an existing `dramatis-artifacts/` further up the tree
(override with `DRAMATIS_FORGE_HOME` or `--home`). That folder can be its own repository,
for example a dataset repository with large files in Git LFS.

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
./forge persona --adopt <version>  # no network: take replies of an older generator version as current
```

Generation uses the `persona` role of `$DRAMATIS_HOME/endpoints.toml` (`--endpoints` for another
file); there is no fallback to `chat`, a missing role is an error. Models billed per request run 3 calls at once by default (`--jobs`); on HTTP 429 every
worker waits together. A persona that fails a gate is not written. Reports go to
`dramatis-artifacts/<pack>/persona/` (`REPORT.md`, `report.json`, `STALE.md`). Each reply is
cached under `persona/raw/`; rebuilding the folio drops the personas, and running
`./forge persona` again restores them from that cache with no model calls.

## Refresh the wording

```sh
./forge wording        # offline: stamp the pack's current wording into the existing folio
```

A rebuild empties the persona table; wording is only text, so changing a pack's wording (UI
and in-world strings, pronoun forms) does not need one. `wording` rewrites the folio's
manifest in place and recomputes its fingerprint; personas stay.

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
export DRAMATIS_FORGE_HOME=/path/to/dramatis-artifacts
export DRAMATIS_FOLIO=$DRAMATIS_FORGE_HOME/<pack>/<pack>.folio
./target/release/dramatis-cli inspect
./target/release/dramatis-cli search "<query>" --top-k 6
./target/release/dramatis-cli evaluate \
    --suite $DRAMATIS_FORGE_HOME/<pack>/evals/structural.queries.jsonl \
    --markdown $DRAMATIS_FORGE_HOME/<pack>/reports/RESULTS.md
./target/release/dramatis-cli bench
```

## Endpoints and keys

`$DRAMATIS_HOME/endpoints.toml` lists profiles (an address, a wire protocol, models with prices)
and roles that point at them. The app, the terminal client and `forge persona` read the same file.
Keys are never in it: each profile's key is in the login keychain (service `dramatis`, account
`profile.<name>`), added in the app's settings.

```toml
[[profile]]
name = "example"
base_url = "https://api.example.com/v1"
wire_api = "chat"                # "chat" | "responses" | "messages" (Anthropic-style)

[[profile.model]]
id = "model-a"
forced_tool = true               # honours a forced tool_choice
context_window = 128000          # tokens; second trigger for rolling a segment
price_in = 1.0                   # only the ratios to price_in are used
price_out = 4.0
billing = "request"              # "token" (default) | "request": a flat fee per call
native_tools = false             # true (default) | false: the endpoint emulates function calling
vision = false                   # false: pictures are described in words, never sent
max_image_bytes = 90000          # a picture over this many base64 bytes is shrunk first

[roles]
chat = { profile = "example", model = "model-a" }       # person turns
host = { profile = "example", model = "model-a" }       # optional: the host; else chat
persona = { profile = "example", model = "model-a" }    # forge persona
```

Pictures pasted into a chat are scaled to a longest side and sent to the model. Leave `vision`
out and an endpoint that refuses a picture is remembered until the next start; set
`vision = false` for a model or proxy that cannot take images. Set `max_image_bytes` for an
endpoint that caps the size of a request.

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

The world (channels, memories, usage, and your profile: name, birthday, bio, avatar) is a save,
one file: `$DRAMATIS_HOME/saves/<name>.sqlite`, and `current_save` in the home directory names the
one a process opens when it starts. A save can be encrypted with a password (`/password on` here, Settings in the app). The
password lives only in memory: it is not written anywhere and cannot be recovered, so a forgotten
one means the save cannot be opened. Only the save is encrypted; `endpoints.toml`, the folio, the
media cache and group avatars are not, and API keys stay in the keychain. Turn encryption on, change
it or off while no other process has the save open. When the current save is encrypted the
terminal asks for the password with hidden input (three tries), the app shows a lock screen. The
terminal and the app share the saves, settings and endpoints, and may be open together (writes wait
for each other, and each sees the other's new messages within a couple of seconds). Each process
keeps the save it started with: switching in one does not move the other until it is restarted,
and deleting a save another process has open loses what that process writes after. Use
`--home <dir>` for another home directory, `--fresh` for a throwaway world (a temporary home that keeps your endpoints and
parameters and is deleted at exit), and `--user <name>` to set the user's name. On a terminal
the line is edited with arrow keys, Home/End and Up/Down history, and wide (CJK) characters count by their display width;
`--plain`, or piped input, reads plain lines with no prompt.

A turn that ends without a reply leaves one line in `$DRAMATIS_HOME/logs/dramatis.log` (time, channel, speaker, model and the first 300 characters of what came back; never keys or the prompt). The file moves to `dramatis.log.1` at about 1 MB, replacing the older one.

| Command | What it does |
| --- | --- |
| `/list [words]` | persons with a persona, and whether the host is available; words: `enabled`, `frozen`, `disabled`, `talked`, `never`, `persona`, or part of a name |
| `/talk <person>` | open the direct channel; an id, display name, other form or alias |
| `/group <p1> <p2> ...` | start a group channel |
| `/host` | switch to the host's channel |
| `/go <n>` | take choice n from the list the host just offered |
| `/history [n]` | reprint the last n messages (default 10), numbered |
| `/reply <n> <text>` | answer message n of the last `/history`; the model is told what it answers |
| `/forward <n> <person\|host\|fav>` | pass message n on to a person or the host, who answers it, or keep it in the favorites |
| `/fav` | open the favorites: saved messages and your own notes, answered by no one; `/delete <n>` removes one |
| `/members`, `/topic [text]` | who is in the group (you first); describe the group |
| `/sources [n]` | list in full the sources of the n-th last reply that has any |
| `/search <query>` | corpus retrieval as the current speaker would see it; no model call |
| `/memories [person]` | numbered memories; `/forget <n>`, `/restore <n>` and `/edit <n> <text>` change them |
| `/save` | list saves; `new`, `switch`, `copy`, `rename`, `delete`, `export`, `import` |
| `/away [on\|off]` | away mode: nobody comes to you and nothing is sent until you turn it off or write a message (in the app: the Away switch in the left drawer and the tray menu) |

`/help` lists all commands, including their short forms. `/quit` or Ctrl-D leaves.

Tab completes command names, the fixed words some commands take (`/tier`, `/quiet`, `/events`, `/away`,
`/lang`, `/library`, `/read`, `/chats`, `/storage`, `/password`), person names (`/talk`, `/group`,
`/invite`, `/remove`, `/who`, `/mode`, `/memories`) and save names (`/save`).

Anything else is sent to the current channel (`//text` sends a message that starts with a slash). In a group, `@name` addresses that member.
Enter starts a new line and Alt-Enter (Option set as Meta on macOS) sends; a line that starts with a slash is a command and Enter runs it. When the
chat opens and after each message the terminal runs what the app runs on a timer (idle
conversations are wrapped up, due openings written), so a person who has something to say shows up
then; there is no background loop. Replies stream as they are written; scene lines
are dimmed, citation marks are hidden and the cited sources are listed after the reply, tool
actions get one dim line, a dim line says who is typing while a turn runs, and another gives
the call count and time. The prompt shows the channel and then your own name. Colour is off when
stdout is not a terminal or `NO_COLOR` is set. A failed call prints its error and the session
goes on. Ctrl-C while a reply is pending cancels the wait (the message you sent stays in the
history); at the prompt it clears the line, and Ctrl-D leaves.

## Usage limits

Calls are counted in one rolling one-hour window. A call billed per request counts 1; a call
billed per token counts by its size. The intensity tier (`/tier`, or the system drawer in the app)
sets how many calls an hour are allowed, and `ultra` has no window. It also sets how often people come by and how many daily events there are, within fixed ceilings. When the window is full new
calls wait until it releases; a turn you start yourself is never refused before then.

## Writing a pack

The contract is `forge/src/dramatis_forge/pack.py`: a module `packs/<name>/__init__.py`
that defines `PACK = Pack(...)` with seed sets, routes to page parsers, inline markup rules,
identity rules, chunking policy, figures, text and stopwords. Missing text keys fall back to
the English defaults in `forge/src/dramatis_forge/text.py`.

## Licence

Code: Apache-2.0, copyright 2026 [alkali](https://github.com/alkalimc). A corpus built with this tool is not covered by that licence; it carries
whatever terms its source has, and the attribution report gives the per-page provenance
needed to honour them.

## Desktop app

macOS 13+ on Apple Silicon only, for now. Node and pnpm as in [Requirements](#requirements).

```sh
cd ui && pnpm i                              # once
pnpm dev                                     # terminal 1: Vite on 127.0.0.1:5173
cd engine && cargo run -p dramatis           # terminal 2: debug app, loads the dev server
```

A debug build loads the UI from the dev server; nothing needs to exist in `ui/dist`. A build
that embeds the UI (`--features custom-protocol`, which `pnpm tauri build` enables) needs
`pnpm build` in `ui/` first.

The app follows a chat client's layout: a conversation list on the left (search, pinned and
archived conversations, unread counts, a right-click menu per row) and the conversation on the
right, with a side panel for a person's or a group's profile; a person's name or avatar in a chat
opens their page as a card over the conversation. You can set your own name, birthday,
bio and avatar, give a group a name and an avatar, attach pictures with the paperclip or paste them into the input line, and open a
reply's sources to see the original passage and its page. Enter inserts a new line and Cmd-Enter
sends. The roster has quick filters (mode, talked to or never talked, has a persona). Saves are managed on the saves page (new, switch, copy, rename, delete, export, import; the same page
is shown after logging out) and the password
setting is under privacy and security.

Settings has an About page: the version, the author, links to the repository and the issue
tracker, and an update check. The automatic check is off until you say yes: the first run (and a
home with no answer yet) asks once, in the app and in the terminal. When on, it asks the GitHub
Releases API which version is the newest (`GET /repos/alkalimc/dramatis/releases/latest`) at most
once a day, and also when you press Check now. The answer lives in `$DRAMATIS_HOME/update_check`
and the last result in `update_cache.json`. It only tells you and links the release page; nothing
is downloaded or installed. The terminal has the same in `/about`, which prints the release page when
a newer version exists, and prints `dramatis <version> · by alkali · <repository>` at start.

The host has a page like a person's: click its avatar or name in a chat, or use the panel button
of its channel (`/who host` in the terminal). Each day in a conversation starts with a date line,
and a group without a name of its own is titled and pictured with you first, then its members in
the order they joined.

The browser preview (`pnpm dev` without the app) shows the neutral English text. To preview a
corpus's own wording, export it once and reload; the file is git-ignored:

```sh
cd forge && DRAMATIS_PACKS=<dir holding packs/> ./forge report wording ../ui/wording.local.json --pack <name>
cd ui && pnpm dev                            # the mock picks up ui/wording.local.json
```

`ui/src/api/gen.ts` is generated from `engine/crates/api`; never edit it. After changing
a command or event type:

```sh
cd engine && cargo test -p dramatis --test bindings   # rewrites ui/src/api/gen.ts
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

The app's icon is a chess knight, drawn in `engine/bins/dramatis/icons/icon.svg`; the macOS menu
bar uses the black silhouette `tray.png` as a template image. To build with other icons, point
`bundle.icon` at your own files with `--config`:

```sh
cd engine/bins/dramatis && npx --prefix ../../../ui tauri build --bundles app \
  --config '{"bundle":{"icon":["/path/32x32.png","/path/128x128.png","/path/128x128@2x.png","/path/icon.icns"]}}'
```

`tauri icon <square-png-or-svg> -o <dir>` generates such a set from one image.

The first run of each signed program asks once more; choose Always Allow and it holds across
rebuilds. A key saved before signing keeps its old access list, so save it again (profile
settings, or delete and add it) if a prompt keeps coming back.

## Maintain

Routine upkeep, in the order it usually happens:

- Update the corpus: `./forge sync`, read `samples/INDEX.md`, then `./forge baseline accept`.
- Restore personas after any rebuild: `./forge persona` (no model calls while the cache in
  `persona/raw/` is current); `./forge persona --stale` lists those whose material changed.
- Change only the pack's wording: `./forge wording`, no rebuild.
- Back up or move a save: with the app and terminal closed, copy the one file in
  `$DRAMATIS_HOME/saves/`, or use `/save export <name> <path>` while they are open. An encrypted
  save stays encrypted and keeps its password. To start over, move `saves/` and `current_save`
  aside (do not delete them); the next start creates an empty save.
- Checks: `cd forge && make test lint && make contract`; `cd engine && cargo fmt --check && cargo
  clippy --workspace --all-targets -- -D warnings && cargo test --workspace`; `cd ui && pnpm exec
  tsc --noEmit && pnpm exec vitest run`.
