"""dramatis-forge — the offline half of dramatis.

The forge turns a collaborative wiki into two artifacts the runtime consumes:

    wiki --harvest--> *.archive --corpus--> *.folio
             *.rawcache (local)      |--> persona prompts (into the folio)
                                     '--> eval suites (published separately)

Everything domain-specific lives in a *pack* (`packs/<domain>/`). The code under
`dramatis_forge/` never names a game, a character, or a wiki template: it owns
mechanism, the pack owns rules. `pack.py` is the whole of that contract.
"""

__version__ = "0.2.0"
