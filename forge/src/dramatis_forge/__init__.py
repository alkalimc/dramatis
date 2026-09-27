"""dramatis-forge — the offline half of dramatis.

The forge turns a collaborative wiki into the artifacts the runtime consumes:

    wiki --sync--> *.archive + *.rawcache --build--> *.folio, evals/, samples/, reports/

`sync` (harvest.py) is the only part that makes requests; `build` (normalize.py,
corpus.py, evals.py, report/) is offline and deterministic.

Everything domain-specific lives in a *pack* (`packs/<domain>/`). The code under
`dramatis_forge/` never names a game, a character, or a wiki template: it owns
mechanism, the pack owns rules. `pack.py` is the whole of that contract.
"""

__version__ = "0.2.0"
