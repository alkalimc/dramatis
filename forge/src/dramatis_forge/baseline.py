"""Accepted baselines: the sizes guards G1 and G4 compare against.

Expected counts are measurements, so they are never written into code. A person reviews
a run, then `forge baseline accept` records what that run measured — seed-set sizes, the
person count, pages per form kind — together with the watermark and time it was taken.
Later runs compare against the file: growth is reported low (accept again once reviewed),
shrinkage high. With no file there is exactly one low finding per guard saying so.
"""

from __future__ import annotations

import datetime as dt
import json
from dataclasses import dataclass, field
from pathlib import Path

from .config import BASELINES_FILE
from .archive import Archive


@dataclass(frozen=True)
class Baseline:
    seeds: dict[str, int] = field(default_factory=dict)
    identity: dict[str, int] = field(default_factory=dict)
    accepted_at: str = ""
    watermark: int = 0


def path_for(archive: Path) -> Path:
    """The baseline file sits beside the archive it describes (`Paths.baselines`)."""
    return archive.with_name(BASELINES_FILE)


def load(archive: Path) -> Baseline | None:
    """The accepted baseline for the archive at `archive`, or None if none was accepted."""
    path = path_for(archive)
    if not path.exists():
        return None
    data = json.loads(path.read_text(encoding="utf-8"))
    return Baseline(
        seeds={k: int(v) for k, v in data.get("seeds", {}).items()},
        identity={k: int(v) for k, v in data.get("identity", {}).items()},
        accepted_at=str(data.get("accepted_at", "")),
        watermark=int(data.get("watermark", 0) or 0),
    )


def measure(archive: Archive) -> Baseline:
    """What the archive holds now, in the shape guards compare against."""
    identity = {"persons": archive.count("persons")}
    for kind, n in archive.db.execute(
        "SELECT kind, COUNT(*) FROM forms WHERE kind<>'canonical' GROUP BY kind ORDER BY kind"
    ):
        identity[f"form:{kind}"] = n
    return Baseline(
        seeds=dict(sorted(archive.seed_counts().items())),
        identity=identity,
        accepted_at=dt.datetime.now(dt.UTC).isoformat(timespec="seconds"),
        watermark=int(archive.get_meta("watermark", 0) or 0),
    )


def accept(archive: Archive) -> tuple[Path, Baseline]:
    current = measure(archive)
    path = path_for(archive.path)
    path.write_text(json.dumps({
        "accepted_at": current.accepted_at,
        "watermark": current.watermark,
        "seeds": current.seeds,
        "identity": current.identity,
    }, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return path, current
