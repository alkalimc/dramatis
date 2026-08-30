"""Domain packs.

A pack is the only place a game, a wiki, or a character may be named. The engine
under `dramatis_forge/` provides harvesting, the record vocabulary, guards,
identity, chunking, benchmark construction and probes; a pack provides the rules
those mechanisms run on, as one module-level `PACK: Pack`.

See `dramatis_forge/pack.py` for the contract, and `arknights/` for the one that
exists.
"""
