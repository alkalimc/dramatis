"""Estimated values the forge uses, one dataclass per parameter group.

Field names mirror the calibration register's dotted names (`coldstart.picks` is
`Params.coldstart.picks`). The defaults are the register's current values and this is the
only place they are written; use sites take the struct as an argument. Overrides come from
an optional TOML file with the same layout (`[coldstart] picks = 4`); missing keys keep
their defaults.
"""

from __future__ import annotations

import tomllib
from dataclasses import dataclass, field, fields, replace
from pathlib import Path


@dataclass(frozen=True)
class Coldstart:
    #: How many people the host recommends on first run.
    picks: int = 3


@dataclass(frozen=True)
class Speaker:
    #: Lines a speaker label needs, across the whole corpus, before its portraits settle
    #: who it is for every scene.
    portrait_lines: int = 10
    #: Share of those lines (or of one scene's, where the corpus-wide count is too thin) that
    #: must show one person's portrait.
    portrait_share: float = 0.6
    #: Share of a roster scene's owners allowed to be missing from their own memoir scenes
    #: before the identity guard turns from a note into a failure.
    owner_missing_max: float = 0.05


@dataclass(frozen=True)
class Params:
    coldstart: Coldstart = field(default_factory=Coldstart)
    speaker: Speaker = field(default_factory=Speaker)

    @classmethod
    def load(cls, path: Path | None) -> Params:
        if path is None or not path.exists():
            return cls()
        raw = tomllib.loads(path.read_text(encoding="utf-8"))
        out = cls()
        for group in fields(cls):
            given = raw.get(group.name)
            if not isinstance(given, dict):
                continue
            current = getattr(out, group.name)
            known = {f.name for f in fields(current)}
            out = replace(out, **{group.name: replace(
                current, **{k: v for k, v in given.items() if k in known})})
        return out
