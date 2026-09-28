"""Where things live, and the build stamp.

Artifacts are kept **outside the source tree** by default. They are large, and a
working tree that contains them makes every `git status` a lie and every clone a
hazard. The default resolves to a sibling
`artifacts/` directory of the workspace, which is the same place whether you run
the CLI from the repo root, from `forge/`, or from a scratch directory.
"""

from __future__ import annotations

import datetime as dt
import os
import platform
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

from . import __version__

ENV_HOME = "DRAMATIS_FORGE_HOME"
BASELINES_FILE = "BASELINES.json"
_MARKERS = ("artifacts", ".git", "pyproject.toml")


def workspace_root(start: Path | None = None) -> Path:
    """Nearest ancestor that looks like a workspace, else the current directory.

    An existing `artifacts/` wins over a `.git/`, so that a repo checked out
    inside a configured workspace still writes to the workspace's artifact store
    rather than starting a second one.
    """
    here = (start or Path.cwd()).resolve()
    for candidate in (here, *here.parents):
        if (candidate / "artifacts").is_dir():
            return candidate
    for candidate in (here, *here.parents):
        if any((candidate / m).exists() for m in _MARKERS[1:]):
            return candidate
    return here


def forge_home() -> Path:
    env = os.environ.get(ENV_HOME)
    if env:
        return Path(env).expanduser().resolve()
    return workspace_root() / "artifacts"


@dataclass(frozen=True)
class Paths:
    """Every path the forge writes, derived from one root and one pack name."""

    home: Path
    pack: str

    @classmethod
    def for_pack(cls, pack: str, home: Path | None = None) -> Paths:
        return cls(home=(home or forge_home()).resolve(), pack=pack)

    @property
    def pack_dir(self) -> Path:
        return self.home / self.pack

    @property
    def archive(self) -> Path:
        """Normalised structured records. The forge's own source of truth."""
        return self.pack_dir / f"{self.pack}.archive"

    @property
    def rawcache(self) -> Path:
        """Fetched wikitext. Local only, never distributed, never read downstream."""
        return self.pack_dir / f"{self.pack}.rawcache"

    @property
    def folio(self) -> Path:
        """The distributable: chunks, lexical index, roster, aliases, prompts, manifest."""
        return self.pack_dir / f"{self.pack}.folio"

    @property
    def samples(self) -> Path:
        return self.pack_dir / "samples"

    @property
    def evals(self) -> Path:
        return self.pack_dir / "evals"

    @property
    def baselines(self) -> Path:
        """Accepted counts the guards compare against; written by `baseline accept`."""
        return self.pack_dir / BASELINES_FILE

    @property
    def reports(self) -> Path:
        """Generated Markdown: FIGURES, COVERAGE, ATTRIBUTION, NOTICE (and the engine's
        RESULTS, when `dramatis-cli evaluate --markdown` is pointed here)."""
        return self.pack_dir / "reports"

    @property
    def coverage(self) -> Path:
        return self.reports / "COVERAGE.md"

    @property
    def figures(self) -> Path:
        """Measured values documents cite by key instead of quoting."""
        return self.reports / "FIGURES.md"

    def ensure(self) -> Paths:
        self.reports.mkdir(parents=True, exist_ok=True)
        return self


def _git_describe(root: Path | None) -> str:
    if root is None:
        return "unknown"
    try:
        out = subprocess.run(
            ["git", "-C", str(root), "rev-parse", "--short", "HEAD"],
            capture_output=True, text=True, timeout=5, check=False,
        )
        rev = out.stdout.strip()
        dirty = subprocess.run(
            ["git", "-C", str(root), "status", "--porcelain"],
            capture_output=True, text=True, timeout=5, check=False,
        ).stdout.strip()
        if rev:
            return f"{rev}{'+dirty' if dirty else ''}"
    except (OSError, subprocess.SubprocessError):
        pass
    return "unknown"


def _source_repo() -> Path | None:
    """The git checkout this package was imported from, if any.

    Not the workspace root: that directory holds several repositories and is not one.
    """
    for candidate in Path(__file__).resolve().parents:
        if (candidate / ".git").exists():
            return candidate
    return None


@dataclass
class RunInfo:
    """Provenance stamped into the folio (`build`): which code produced this file.

    The reproducibility floor: a result whose artifact cannot be traced to a build is
    not a result.
    """

    stage: str
    pack: str
    pack_version: int
    forge_version: str = __version__
    started_at: str = field(default_factory=lambda: dt.datetime.now(dt.UTC).isoformat(timespec="seconds"))
    code_revision: str = "unknown"
    python: str = field(default_factory=lambda: sys.version.split()[0])
    host: str = field(default_factory=lambda: f"{platform.system()} {platform.machine()}")

    @classmethod
    def create(cls, stage: str, pack: str, pack_version: int) -> RunInfo:
        return cls(
            stage=stage,
            pack=pack,
            pack_version=pack_version,
            code_revision=_git_describe(_source_repo()),
        )

    def as_dict(self) -> dict[str, object]:
        return {
            "stage": self.stage,
            "pack": self.pack,
            "pack_version": self.pack_version,
            "forge_version": self.forge_version,
            "started_at": self.started_at,
            "code_revision": self.code_revision,
            "python": self.python,
            "host": self.host,
        }
