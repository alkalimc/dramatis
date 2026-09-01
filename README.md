# dramatis

Turn a collaborative wiki into retrieval-grounded character agents — an offline corpus
forge and a runtime engine.

The forge harvests a wiki, normalises it into records, and packs a single-file knowledge
base. The engine reads that file and runs the agents. Everything domain-specific lives in
a *pack*; the engine never names a game, a character, or a site.

```
dramatis/
  forge/        Python 3.13 — the offline half
    src/dramatis_forge/     mechanism: harvest · normalize · corpus · evals · probes · report
    tests/                  framework tests; none of them load a pack
  engine/       Rust 2024 — the runtime
    crates/folio/           read a .folio: manifest, units, f16 vectors, roster, aliases
    crates/index/           lexical BM25 ∥ exact dense scan → RRF → confidence
    crates/eval/            suite runner and retrieval metrics
    bins/dramatis-cli/      inspect · search · bench
  docs/         contributor documentation
```

## This is half of a working system

**No pack ships here.** The framework is mechanism: it knows how to harvest a MediaWiki
site, normalise it into records, resolve pages to people, cut retrieval units, pack a
single-file corpus, build benchmark suites and measure itself. It knows nothing about *any
particular* site — which pages to take, how to parse their markup, which templates declare
that two pages are one person, how a character should be addressed.

Those rules are a pack, and supplying one is the work. Point `DRAMATIS_PACKS` at a
directory containing a `packs/<name>/` tree and every stage above becomes available to it;
without one, the CLI has nothing to run.

The contract is `forge/src/dramatis_forge/pack.py` in its entirety. It is deliberately
thin — declarative rule objects plus a handful of parser callables behind one module-level
`PACK` — and there is no plugin base-class hierarchy, because with the seam this narrow
that would be fiction dressed as architecture.

The engine is a **skeleton, deliberately**: retrieval and measurement only. It exists
before the daemon because two freeze conditions — first-token latency and resident memory —
cannot be settled on paper, so the thing that answers them has to run first. The agent
loop, world state and RPC layer are not written; there are no empty crate directories
standing in for them, because a tree of empty crates looks like progress and is not.

Three defects it found on its first run were invisible to reading the code: a stopword that
made BM25 score nearly the whole corpus, SQL rebuilt per candidate count so it never
cached, and a filter applied after truncation that returned nothing. A fourth was worse —
the memory measurement itself was wrong, returning success and a resident size of zero.
**A system call that succeeds is not the same as a correct measurement**, and two freeze
conditions rest on that number.

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
make test             # framework tests, ~1s
export DRAMATIS_PACKS=/path/to/your/rules   # the directory holding packs/<name>/
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

Products land in `../artifacts/` — outside every repository, because they run to hundreds
of megabytes and a working tree is the wrong place for them.

## The engine/pack seam

One rule decides where code goes: **the engine owns mechanism, the pack owns rules.** If a
line contains a wiki template name, a character name, a game term, or a site URL, it
belongs to a pack — and therefore not to this repository. A pack that satisfies the
contract gets rate-limited harvesting, the record vocabulary, the guards, identity
resolution, chunking, folio packing, benchmark construction, the probe runner and the
figure reporter for free.

The seam is also where measurement lives. Documents describing a corpus cite report keys
rather than values: a figure copied into prose outlives the build it came from, and nothing
downstream can tell that it has. `report figures --check` audits a document tree against
the artifact it claims to describe, and reports both retired renderings and references that
resolve nowhere.

Which figures a pack exposes, and which renderings each has retired, are supplied by the
pack — both are domain knowledge, and a retired-value list is design history.

## Guards, not rule tables

A rule table's characteristic failure is not being wrong — it is being wrong *quietly*.
Output still appears and counts still look plausible. So every assumption in the pipeline
is an assertion with a measured baseline and a place to complain:

| Guard | Checks |
| --- | --- |
| G1 | seed-set drift (graded three ways), and produced-versus-stored reconciliation |
| G2 | markup the rules did not predict |
| G3 | an in-scope page that produced nothing |
| G4 | page-to-person identity invariants |
| G5 | chunking coverage, size and redundancy |
| G6 | **the artifact agrees with itself**: tables ↔ manifest ↔ report |

Severity means one thing: **could content have been lost?** A high-severity finding blocks
the design freeze. A low-severity one must be *attributed* — an unexplained low-severity
count is a high-severity finding in waiting.

G6 is the only one that does not check the corpus, and it exists because the other five
cannot catch the failure that actually happened: a build whose own records disagreed about
how many findings it had. A file that asserts "I have no high-severity problems" needs
something to verify that assertion, and for a while nothing did. G1's drift check is graded
rather than binary for the same reason — "shrank" and "grew by one" are different facts, and
collapsing them produced a false alarm shaped exactly like a real data-loss bug.

## Licence

Apache-2.0. **No third-party code is vendored here** — `NOTICE` declares ordinary
dependencies only. An earlier design vendored two crates for a command-execution
subsystem; that subsystem became an optional, off-by-default extension, and carrying
thousands of lines of vendored code for a subsystem that is absent by default was a cost
with nothing on the other side.

The corpus this builds is **not** covered by that licence: it derives from a
volunteer-maintained wiki whose editorial contributions are CC BY-NC-SA, over fiction
belonging to its rights holder. Anything redistributed needs per-page attribution with a
revision id, a non-commercial restriction, and a takedown contact.
