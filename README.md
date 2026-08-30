# dramatis

Turn a collaborative wiki into retrieval-grounded character agents — an offline corpus
forge and a runtime engine.

The forge harvests a wiki, normalises it into records, and packs a single-file knowledge
base. The engine reads that file and runs the agents. Everything domain-specific lives in
a *pack*; the engine never names a game, a character, or a site.

```
dramatis/
  forge/        Python 3.13 — the offline half
    src/dramatis_forge/     mechanism: harvest · normalize · corpus · evals · probes
    packs/arknights/        rules: the only place a game may be named
    tests/                  118 tests: unit · packs · pipeline
  engine/       Rust 2024 — the runtime
    crates/folio/           read a .folio: manifest, units, f16 vectors, roster, aliases
    crates/index/           lexical BM25 ∥ exact dense scan → RRF → confidence
    bins/dramatis-cli/      inspect · search · bench
  docs/         contributor documentation
```

The engine is a **skeleton, deliberately**: retrieval and measurement only. It exists
before the daemon because two freeze conditions — first-token latency and resident memory —
cannot be settled on paper, so the thing that answers them has to run first. The agent
loop, world state and RPC layer are not written; there are no empty crate directories
standing in for them, because a tree of empty crates looks like progress and is not.

## Why one repository for two languages

`.folio` is a contract written by Python and read by Rust. Keeping both here means a
schema change, its writer, and its reader move in a single commit. Splitting them would
turn every format change into a two-repository dance for no benefit.

The desktop client is a separate repository because its licence is different and
incompatible with this one.

## Getting started

```sh
cd forge
make install          # venv + editable install + a macOS path-file workaround
make test             # 118 tests, ~1s
./forge --help
```

Then, to build everything from scratch:

```sh
./forge harvest sync    # the only stage that touches the network; resumable
./forge corpus build
./forge evals build
./forge probe run
```

And to query what that produced:

```sh
cd ../engine
cargo build --release
./target/release/dramatis-cli inspect
./target/release/dramatis-cli search "源石技艺"
./target/release/dramatis-cli bench     # latency distribution and resident memory
```

Products land in `../artifacts/` — outside every repository, because the archive is 80 MB
and a working tree is the wrong place for it.

## The engine/pack seam

One rule decides where code goes: **the engine owns mechanism, the pack owns rules.** If a
line contains a wiki template name, a character name, a game term, or a site URL, it
belongs to a pack. `forge/src/dramatis_forge/pack.py` is the whole contract; a pack that
satisfies it gets rate-limited harvesting, the record vocabulary, five guards, identity
resolution, chunking, folio packing, benchmark construction and the probe runner for free.

## Guards, not rule tables

A rule table's characteristic failure is not being wrong — it is being wrong *quietly*.
Output still appears and counts still look plausible. So every assumption in the pipeline
is an assertion with a measured baseline and a place to complain:

| Guard | Checks |
| --- | --- |
| G1 | seed-set drift, and produced-versus-stored reconciliation |
| G2 | markup the rules did not predict |
| G3 | an in-scope page that produced nothing |
| G4 | page-to-person identity invariants |
| G5 | chunking coverage, size and redundancy |

Severity means one thing: **could content have been lost?** A high-severity finding blocks
the design freeze. A low-severity one must be *attributed* — an unexplained low-severity
count is a high-severity finding in waiting.

## Licence

Apache-2.0. Vendored third-party code is declared in `NOTICE`.

The corpus this builds is **not** covered by that licence: it derives from a
volunteer-maintained wiki whose editorial contributions are CC BY-NC-SA, over fiction
belonging to its rights holder. Anything redistributed needs per-page attribution with a
revision id, a non-commercial restriction, and a takedown contact.
