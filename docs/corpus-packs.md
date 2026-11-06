# Writing a corpus pack

A corpus pack is the source material the app talks about, plus the rules, wording and
personas that turn it into something the app can use. This document is enough to write one,
together with the toy pack in `forge/tests/toypack/packs/toy/__init__.py`. Every command
below was run against that toy pack.

## What a pack is

The app (the engine in `engine/` and the UI in `ui/`) ships no corpus. A corpus is one file,
a `.folio`, produced by the forge (`forge/`, Python) from a **pack**: a Python module that
declares, as data and a few small parser functions, everything specific to one source.

The source must be a MediaWiki site (the forge talks to its API). The split is fixed:

- The forge owns mechanism: fetching, archiving, normalising, guards, chunking, the lexical
  index, packing the folio, reports, persona generation.
- The pack owns rules: which pages to take, how to read them, who counts as a person, how
  to word things, how a persona is written.

The result is a `<pack>.folio` that the app opens. The app never reads the pack.

## Parts of a pack

A pack is the module `packs/<name>/__init__.py` defining a module-level `PACK = Pack(...)`
(`forge/src/dramatis_forge/pack.py` is the contract). The forge finds it by importing
`packs.<name>`, `dramatis_packs.<name>` or `<name>`, so the directory that contains
`packs/` must be on `DRAMATIS_PACKS` or `PYTHONPATH`.

The toy pack is a single file (`forge/tests/toypack/packs/toy/__init__.py`, about 190
lines). A real pack may add two more things next to it:

```text
packs/<name>/
  __init__.py     PACK = Pack(...)               required
  persona.py      GENERATOR = Generator(...)     optional: persona prompts and gates
  templates/*.md  attribution/notice templates   optional: filled by the forge
```

The fields of `Pack`, with what the toy pack does with each:

| Field | What it is | In the toy pack |
| --- | --- | --- |
| `name`, `version` | Pack name, and its own `major.minor.patch` release, stamped into the folio as `pack_version`. | `"toy"`, `"1.0.0"` |
| `wiki` (`WikiConfig`) | API address, a contact for the User-Agent, URL patterns (`source_url`, `history_url`, `revision_url`, `avatar_url`, player and host picture URLs), request rate. `{page}` and `{revid}` are substituted. | all `https://toy.invalid/...` |
| `seeds` (`SeedSet`) | Closed sets of page titles: enumerated from the site (`enumerate=`) or hand-listed (`fixed=`). `roster=True` marks the sets whose pages are people. | `story` (category members), `people` (fixed, roster) |
| `routes` (`Route`) | Which parser reads which pages: by seed set, optionally narrowed by exact `title` or `title_prefix`. `min_yield` is the least share of a page's visible letters its records must keep. | `story` goes to `parse_story`, `people` to `parse_prose` |
| `followups` | Pages whose body is found only after fetching another page. | none |
| `inline` (`InlineRules`) | How wiki templates and macros become plain text: `drop`, `text_param`, `content`, `literal`, `macros`, `drop_sections`, and so on. Anything outside the table is reported, not guessed. | drops `cite`; turns `${player}` into `{user}` |
| `identity` (`IdentityRules`) | Maps page identity to person identity: which pages are alternate forms of a person. Only signals the site states explicitly may be used. | `{{AltForm|Alice}}` makes "Alice (Winter)" a form of Alice |
| `chunking` (`ChunkPolicy`) | The retrieval-unit templates, their size budgets, header lines, labels and separators. | `lore`, `dialogue`, `profile` |
| `roster` (`RosterRules`) | Per-person fields derived from dossier material: birthday, topic terms, affiliation, a one-line reason shown on first run. | reads `born`, `home`, `role` |
| `wording` | UI and in-world strings, keyed like the client's i18n keys. See below. | `host.name`, `user.title` |
| `text` | Wording for the forge's own generated files (reports), keyed as in `dramatis_forge/text.py`. | two keys |
| `figures` (`FigureSpec`) | Measured quantities with optional pass/fail targets. | thirteen, e.g. `units > 0` |
| `finding_notes` | Reviewed explanations for expected low-severity guard findings. | one, for pages with no script |
| `publication` | Credits shown in the app and used by notice templates. | unset |
| `scene_marker`, `year_offset`, `user_placeholder`, `stopwords`, `tokenizer` | Manifest values the runtime reads. | `("*","*")`, `100`, `"{user}"`, `("the","a")`, unset |
| `tables`, `coverage`, `alias_pages`, `audit` | Structured-table queries, coverage annotations, alias-only pages, document-audit settings. | mostly empty |

### The toy pack's source

The toy source is invented. `story` pages are dialogue scripts, one line per utterance:

```text
{{Scene|category=Main|group=Chapter One}}
> The harbour is quiet.
Alice: Good morning, ${player}.
[caption] Later that day
[choice] Wait with them | Walk away
```

`people` pages are prose. `{{Info|born=03-14|home=North Cape|role=...}}` carries the
dossier, and `{{AltForm|Alice}}` marks another form of a person. A parser is a function from
a `PageContext` to records (`Scene`, `Line`, `Choice`, `Dossier`, `Lore`, ...):

```python
def parse_prose(ctx: PageContext) -> Iterator[Record]:
    info = RE_INFO.search(ctx.wikitext)
    if info:
        fields = dict(p.split("=", 1) for p in info.group(1).split("|") if "=" in p)
        yield Dossier(page=ctx.title, fields=fields, revid=ctx.revid)
    for section in ctx.clean.split_sections(RE_INFO.sub("", ctx.wikitext), []):
        yield Lore(page=ctx.title, path=section["path"], text=section["text"],
                   revid=ctx.revid)
```

Parsers see only the page text (no store, no network), so changing a rule means re-running
`forge build`, not re-fetching. A parser that finds nothing on purpose calls
`ctx.empty("reason")`. One that yields nothing silently is reported by guard G3 as a failure.
The toy has two such pages on purpose: `Interlude` (explained) and `Silent` (not).

## Wording

`Pack.wording` is a flat mapping of key to string. The forge copies it into the folio
manifest, and the app reads it at run time: the UI, the terminal client, the agent harness
and the tool descriptions all look up their text there and fall back to neutral English
for any missing key. A pack can therefore start with an empty mapping and override only
what its fiction needs. The toy sets two keys:

```python
wording={"host.name": "Keeper", "user.title": "Captain {name}"}
```

### Finding the full key list

The list is the file `forge/src/dramatis_forge/wording_keys.txt` (one key per line,
sorted, currently 1308 keys). It is generated, not hand-written. A test collects the keys
from the places that read them and fails if the file differs:

| Source | Keys it defines |
| --- | --- |
| `ui/src/i18n/en.ts` | UI text, with the English defaults |
| `engine/crates/agent/src/text.rs` (`DEFAULTS`) | `harness.*`, `host.*`, `boilerplate.*`, `weekday.*`, `daypart.*` and similar: text the model sees |
| `engine/bins/dramatis-cli/src/chat/text.rs` (`DEFAULTS`) | `cli.*`: the terminal client |
| `engine/crates/api/src/tools.rs` and `engine/crates/api/tests/tools.snapshot.json` | `tool.*`: tool and parameter descriptions |

To see the English default for a key, search for it in those files. To add, rename or remove
a key in the engine or UI, update the pack, then regenerate the list from `forge/`:

```sh
UPDATE_SNAPSHOTS=1 python -m pytest -q tests/test_wording_keys.py
```

Without `UPDATE_SNAPSHOTS` the same test fails when `wording_keys.txt` and the sources
disagree. A pack can also track how many keys it leaves to the defaults with a figure:
`FigureSpec("wording.missing", "derived:wording_missing", target="== 0")` (the toy omits it).

### How keys are named

Keys are dotted and grouped by the first segment. The groups with the most keys:

| Prefix | Used for |
| --- | --- |
| `cli.*` | the terminal client (usage text, prompts, help) |
| `harness.*` | sentences the agent harness puts in a model session |
| `endpoints.*`, `saves.*`, `storage.*`, `quota.*`, `privacy.*` | settings screens |
| `tool.*` | descriptions of the tools a model may call |
| `host.*`, `user.*`, `person.*`, `org.*`, `pronoun.*` | the corpus's own names: the assistant, the user, "person", the organisation, pronouns |
| `weekday.*`, `daypart.*`, `clock.*`, `locale` | how time is said; `clock.in_world` shows the in-world date |
| `about.*`, `credit.*`, `app.*`, `setup.*`, `roster.*`, `chatlist.*`, ... | UI screens |

The in-world names (`host.name`, `org.name`, `person.noun`, `user.title`, `pronoun.*`) are
the ones every pack should set first.

### Placeholders

Placeholders are `{field}`; unknown ones are left as written.

- UI strings: the caller fills `{name}`, `{count}` and similar (read the English default for
  the fields a key takes). In every UI template `{host}`, `{org}`, `{person}` and `{people}`
  are available and resolve to `host.name`, `org.name`, `person.noun` and
  `person.noun_plural`, so a sentence need not hard-code the corpus's nouns.
- Harness strings (`harness.*`, `host.*`): `{user}` is the user's name (or `user.you` when
  none is set) and `{host}` is `host.name`, in every key. Other fields vary per key, such as
  `{name}`, `{text}`, `{when}`.
- Several `boilerplate.*` keys hold one rule per line: phrases that mark a reply as an
  endpoint's canned refusal or error rather than a character. A pack adds its own language.
- `user_placeholder` (a `Pack` field, default `{user}`) is how the corpus text itself writes
  the user's name. The app substitutes it when it builds model-visible text and never edits
  the stored corpus. The toy rewrites `${player}` to `{user}` in `inline.macros`.

### Rules for text a model sees

The public checks are the persona gates (below) and the default persona generator's
prompt, which asks for nothing outside the fiction, no behaviour imperatives and no
numbers. The `harness.*` English defaults in `engine/crates/agent/src/text.rs` show the
register: second-person description of the situation, no claims about what a character
knows. Write model-facing wording the same way, saying positively what is true or what to
do. A pack should add checks for its own language; the forge cannot know them.

## Building with forge

The forge is a Python 3.13 package (`forge/`). Setup and the two main commands are in the
README. From `forge/`:

```sh
make install                          # venv + editable install
export DRAMATIS_PACKS=/path/to/rules  # directory that contains packs/<name>/
export DRAMATIS_PACK=toy              # or pass --pack toy to every command
./forge --help
```

`./forge` is a wrapper that runs `python -m dramatis_forge` with `forge/src` and
`DRAMATIS_PACKS` on `PYTHONPATH`. Calling `python -m dramatis_forge` directly works too, if
the package is importable; it adds `DRAMATIS_PACKS` to `sys.path` itself.

| Command | Network | What it does |
| --- | --- | --- |
| `forge sync [--full] [--dry-run] [--no-rescope] [--rate N]` | yes | Bring the archive level with the site, then run `build`. First run fetches everything; later runs follow the change feed. Resumable. |
| `forge build [--no-samples]` | no | normalize, then folio, evals, samples and reports. Re-run after any rule change. |
| `forge baseline accept` / `show` | no | Record or compare the counts the guards check against. |
| `forge wording` | no | Stamp the current wording, credits and host picture into the existing folio without a rebuild. |
| `forge report samples\|figures\|coverage\|attribution\|inspect\|wording` | mostly no | Regenerate one report; see below. |
| `forge persona [...]` | yes (a model) | Generate, gate and write personas. See Personas. |

Common options: `--pack/-p <name>` (or `DRAMATIS_PACK`) and `--home <dir>` (or
`DRAMATIS_FORGE_HOME`). The artifact root defaults to a `dramatis-artifacts/` directory
beside the repository (or an existing one further up); each pack gets `<home>/<pack>/`.

### Building the toy pack

The toy wiki is a fake (`toy.invalid`), so `forge sync` cannot fetch it. The test fixture
fills the archive from in-memory pages (`FakeWiki` in `forge/tests/conftest.py`); after that
the real `build` runs. A pack of your own starts with `forge sync` against the real site
instead. (`make contract` in `forge/` builds the toy folio for the engine's contract tests.)

To run the commands on the toy, seed an archive with that fixture (a script that imports
`conftest` and calls `sync(FakeWiki(), pack, paths)`), then:

```sh
export DRAMATIS_PACKS=$PWD/tests/toypack
./forge build --pack toy --home /tmp/toyhome
./forge baseline accept --pack toy --home /tmp/toyhome
```

`build` prints the record counts, the folio summary (`units 15 · dialogue 8 · lore 5 ·
profile 2`), the structural eval suite, the guard table and the figures, then writes:

```text
<home>/toy/toy.archive            normalised records (the forge's source of truth)
<home>/toy/toy.rawcache           fetched wikitext; local only, never distribute
<home>/toy/toy.folio              the corpus the app opens
<home>/toy/evals/structural.queries.jsonl, structural.manifest.json
<home>/toy/samples/INDEX.md       review sample (plus one folder per route)
<home>/toy/reports/COVERAGE.md, FIGURES.md
<home>/toy/BASELINES.json         after `baseline accept`
```

`build` exits with status 1 when a guard finding is high severity or a figure misses its
target. On the toy this happens by design: the page `Silent` yields nothing and gives no
reason, so G3 reports it and `guards.high == 0` fails.

## Checking and reports

Every build checks itself. The checks are guards, each with a name; a high-severity finding
means content may have been lost and fails the build (exit status 1).

| Guard | Checks |
| --- | --- |
| G1 | Seed-set sizes against the accepted baseline; produced against stored counts. |
| G2 | Markup or templates the inline rules did not predict. |
| G3 | An in-scope page that produced no records, or fewer than `min_yield` of its letters. |
| G4 | Page-to-person resolution invariants (the person count against the baseline). |
| G5 | Chunking coverage, size and redundancy. |

Sizes are never declared in code. Review a build's `samples/INDEX.md`, then run
`forge baseline accept`; from then on a shrinking seed set or person count is high severity
and growth is low. A low-severity finding that no `FindingNote` explains is counted by the
figure `guards.unattributed`.

Reports are written under `<home>/<pack>/`. All are generated; do not edit them.

| Command | Writes | Read it for |
| --- | --- | --- |
| `forge report samples` | `samples/INDEX.md` and one folder per route | Sync status, guard tally, markup left in record text, every page the last sync changed, and boundary cases (first, largest, smallest, first that produced nothing). Each page appears as fetched (`.1-source.wikitext`), as records (`.2-records.json`) and as archived text (`.3-archived.md`). |
| `forge report inspect "<title>"` | `samples/inspect/` | The same three views for one page. |
| `forge report figures [--show]` | `reports/FIGURES.md` | Each `FigureSpec` value with pass or fail against its target. Exits 1 on a failed target. |
| `forge report figures --check <dir>` | nothing | Audits a Markdown tree for dangling links, undefined ids and unknown backticked figure keys. |
| `forge report coverage` | `reports/COVERAGE.md` | Record counts by kind, and the pack's `coverage` rows: what was taken, what was excluded, why. |
| `forge report attribution [--editors]` | `reports/ATTRIBUTION.md`, `NOTICE.md` | Per-page provenance. Written only when the pack has `templates/*.md`, and refused if any unit lacks a source revision. |
| `forge report wording <file.json>` | the JSON file | The pack's wording as JSON, for the UI's browser preview (development only). |

The `build` also writes `evals/structural.queries.jsonl`: a query suite derived from the
folio itself (for example, a person's form name should reach that person). The engine
scores it with `dramatis-cli evaluate --suite ... --markdown ...` (see the README).

Attribution templates are plain Markdown with `{KEY}` placeholders (`SITE`, `PAGE_ROWS`,
`LICENCE`, `PACK_VERSION` and others); `values()` in
`forge/src/dramatis_forge/report/attribution.py` lists them all.

## The folio format

A `.folio` is one SQLite file. It is built by `forge/src/dramatis_forge/folio.py` (schema)
and read, read-only, by `engine/crates/folio`. A folio is never patched in place; `build`
recreates it (which empties the persona rows).

| Table | Holds |
| --- | --- |
| `chunks` | The retrieval units: `id`, `ord`, `template`, `page`, `revid`, `title`, `header`, `text`, `chars`, and a span (`span_of`, `span_from`, `span_to`) used to widen a hit with its neighbours. |
| `chunks_fts` | The lexical index: pre-segmented tokens in a plain FTS5 column. |
| `persons` | Roster: `person_id`, `primary_page`, `display`, `forms`, `facets`, `material` (characters), `persona_confidence`, `birthday` (`MM-DD` or null). |
| `unit_persons`, `cooccur`, `knowledge_scope`, `topic_terms` | Which persons a unit belongs to, scenes shared by pairs, what a person knows first-hand, topical terms. |
| `aliases` | Alias to target, with a kind (the toy has `Alice (Winter)` to `Alice`, kind `form`). |
| `prompts` | `(subject, slot, body, generator_version)`: persona text and the host's layers. |
| `template_stats` | Per-template size statistics used to calibrate scores. |
| `manifest` | Key to JSON value. |

The manifest keys the app acts on: `format_version` (currently 2; the app refuses any other),
`requires` (capabilities the reader must implement, today `neighbor_expand`; an unknown
requirement is refused), `pack`, `pack_version`, `segmenter`, `stopwords`, `wording`,
`scene_marker`, `user_placeholder`, `clock.year_offset`, `template_shapes`,
`source_url_pattern`, `chunks_by_template`, `build_fingerprint`, and the optional `credits`.
Others (`build`, `built_at`, `size_stats`, `checks`, `redundancy`) are provenance.

Query-side segmentation must match the build side: the folio records the segmenter
(`jieba/<version>` or `char-bigram/1`); `forge build` uses jieba when the `zh` extra is
installed and the bigram fallback otherwise. A toy unit has the header
`[scene] Chapter One · Chapter 1 · Alice, Bob` and the dialogue lines as its text.

The app opens the folio with `Folio::open`, which refuses an unknown `format_version` or
unmet requirement with a message to update the app. A pack does not write any of this by
hand; it is the output of `build`.

## Personas

A person can be spoken to only if the corpus holds all three persona slots for them:
`system`, `tone` and `capability` (`engine/crates/agent/src/persona.rs`, `PERSONA_SLOTS` in
`engine/crates/api/src/corpus.rs`). A roster person without them is treated like a disabled
person. The host needs an `in_world` layer, and a `meta` layer for talk about the
simulation itself. The optional `lines` slot holds a person's own lines, one per line, as
tone references; `forge persona` picks them from the material.

| Slot | Content | Limit (defaults, `persona.*` in `params.toml`) |
| --- | --- | --- |
| `system` | Continuous second-person prose: how they speak, what they care about, who they are | 400 tokens |
| `tone` | Short habits of speech, one rule per line | 3 to 4 rules |
| `capability` | Identity and field for a roster line, no praise | 40 characters |
| `lines` | Their own lines, picked not generated | up to 8, 400 tokens in all |

### Generating

```sh
./forge persona --probe          # a small probe set; one readable file per subject
./forge persona                  # all persons; resumable
./forge persona --stale          # offline: list personas whose material changed
./forge persona --only <name> --force
```

This calls a model through the `persona` role of `$DRAMATIS_HOME/endpoints.toml` (see the
README; there is no fallback to `chat`). One call per person, with their material from
every form. A person with fewer than `persona.min_own_units` (default 1) voice or dialogue
units is skipped, because with no words of their own a generator can only invent a voice.
A reply is written only if it passes three gates:

1. Meta leak: none of the generator's `meta_terms` or `meta_patterns` in any slot a
   person's session carries (only the host's `meta` slot may use them).
2. Structure and budget: complete schema, within the limits above, and the user named only
   by the corpus placeholder.
3. Prescriptive phrasing: no capability claims ("you know everything") and no lists of
   what the person does not know, outside quoted examples of their own words. What a
   person knows comes from retrieval, so a claim in the prompt that disagrees with what
   retrieval returns makes the model invent.

Replies are cached under `<home>/<pack>/persona/raw/`, so rebuilding the folio and running
`forge persona` again restores the personas without model calls. Reports (`REPORT.md`,
`report.json`, `STALE.md`) are in `<home>/<pack>/persona/`.

### The pack's generator

Without `packs/<name>/persona.py` the forge uses a neutral English default. A pack defines
`GENERATOR = Generator(...)` there (`dramatis_forge.persona.Generator`): the system and
user prompts (placeholders `{name}`, `{forms}`, `{material}`, `{prompt_chars}`,
`{capability_chars}`, `{tone_min}`, `{tone_max}`, `{user_rule}`, `{user_placeholder}`), the
host's prompts and `HostSource`, the gate lexicons (`meta_terms`, `meta_patterns`,
`prescriptive_patterns`, `user_name_patterns`), section labels for the material,
`title_kinds` and `line_drops` to reclassify or drop units, and `corrections`. The
generator version is a digest of everything that changes what the model is asked, so
editing a prompt invalidates old replies on its own.

A `Correction` replaces chosen fields of one subject's reply with hand-written text, with
the reason recorded; the corrected text still goes through the gates.

### Hand-written personas

The app also reads `$DRAMATIS_HOME/prompts/<subject>.yaml`, a map of slot to body, and
counts those slots as present (`system`, `tone`, `capability`, one rule per line in
`tone`). This is a local override that wins over the folio; it is not part of a
distributed corpus. This path was read from the code, not run. To ship hand-written
personas in a folio, use `Correction` fields (applied over a generated reply); there is no
separate import command.

## Running the result in the app

The app opens the first of these that exists (`Home::resolve_folio`,
`engine/crates/api/src/home.rs`):

1. `DRAMATIS_FOLIO`, a path to a `.folio`.
2. The corpus chosen in the app, remembered in `<home>/corpus_path` (set from the file
   dialog or by typing the path on the setup screen).
3. The newest `*.folio` directly inside the home folder.

The home folder is `$DRAMATIS_HOME`, else a folder chosen in the app, else `~/.dramatis`.
With none found, the app shows "Open your corpus" and asks for the file. The terminal
client takes `--folio <path>` first (README, "Terminal").

```sh
export DRAMATIS_FOLIO=/path/to/dramatis-artifacts/<pack>/<pack>.folio
cd engine && cargo run -p dramatis-cli --release -- inspect      # corpus summary
cargo run -p dramatis-cli --release -- search "<query>" --top-k 6
cargo run -p dramatis-cli --release -- chat                       # then /list, /talk <person>
```

The About page of the app shows the corpus credits and pack version. To
try changed wording on an existing folio, copy it to a scratch directory and run
`forge wording --home <scratch>` against the copy, then point `DRAMATIS_FOLIO` at the copy.
`forge wording` rewrites only `wording`, `credits`, `host_avatar_url` and the fingerprint.
Changes to other manifest values (`scene_marker`, `year_offset`, `user_placeholder`,
`stopwords`) need `forge build`, which empties the persona table until `forge persona`
restores it from the cache.

A save records the pack name and version it was last opened with. Raise
`Pack.version` when a release changes behaviour, since the app uses it to tell the user the
corpus changed.

## Licence and distribution

- The app and the forge are Apache-2.0. That licence does not cover a corpus. A corpus
  carries the terms of its source material, and the pack author is responsible for having
  the right to redistribute it. Wiki text is commonly under a share-alike licence; a
  game, book or film's own text usually is not free to copy. Read the terms first.
- Do not bundle copyrighted text without permission. When in doubt, distribute the pack
  (rules only, no text) and let each user run `forge sync` themselves.
- `forge report attribution` writes the per-page provenance (page, revision, editor,
  history link) needed to honour attribution terms. Give it templates in
  `packs/<name>/templates/`.
- Keep generated artifacts (`.archive`, `.rawcache`, `.folio`, reports) out of the engine
  repository. The forge writes them outside the source tree by default. The `.rawcache` is
  fetched source text and is never meant to be distributed.
- Credits: set `Pack.publication` and the forge writes `credits` into the folio manifest.

  | `Publication` field | Manifest `credits` key | Shown in the app |
  | --- | --- | --- |
  | `credit_by` | `by` | "Corpus built by {by}" |
  | `credit_url` | `url` | links that line; `https://` only |
  | `project_url` | `project` | "Corpus project" link; `https://` only, hidden if it is this repository |
  | `licence` | `licence` | "Corpus licence: {licence}", or joined with `by` |

  The credits line (Settings, About) appears when `by` or `licence` is set. The pack
  version is shown separately as "Corpus pack {version}". `issue_url` and
  `contact_email` are only for notice templates. The text around these values comes from
  the wording keys `about.corpus`, `about.corpus.by`, `about.corpus.licence`,
  `about.corpus.project` and `about.corpus.version`.
- `wiki.contact` goes into the User-Agent of every request `forge sync` makes. Use a real
  contact, keep `rate` modest, and follow the site's rules for bots.
