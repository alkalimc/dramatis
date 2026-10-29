"""The wording key list the build measures against must match the sources that read keys."""

from __future__ import annotations

import os
import re
from pathlib import Path

from dramatis_forge.report.figures import wording_keys

REPO = Path(__file__).resolve().parents[2]
LIST = Path(__file__).resolve().parents[1] / "src" / "dramatis_forge" / "wording_keys.txt"


def collect(repo: Path) -> list[str]:
    """UI keys, harness keys, terminal keys and tool-description keys, read from where they are defined."""
    ui = (repo / "ui/src/i18n/en.ts").read_text(encoding="utf-8")
    keys = set(re.findall(r'^\s+"([a-z0-9_.]+)":', ui, re.M))
    rs = (repo / "engine/crates/agent/src/text.rs").read_text(encoding="utf-8")
    table = rs.split("const DEFAULTS", 1)[1].split("];", 1)[0]
    keys |= set(re.findall(r'\(\s*"([a-z0-9_.]+)",', table))
    cli = (repo / "engine/bins/dramatis-cli/src/chat/text.rs").read_text(encoding="utf-8")
    table = cli.split("pub const DEFAULTS", 1)[1].split("];", 1)[0]
    keys |= set(re.findall(r'\(\s*"([a-z0-9_.]+)",', table))
    tools = (repo / "engine/crates/api/src/tools.rs").read_text(encoding="utf-8")
    keys |= {f"{ns}.{name}"
             for ns, name in re.findall(r'spec::<\w+>\("(tool(?:\.host)?)", "(\w+)"', tools)}
    return sorted(keys)


def test_key_list_matches_sources():
    found = collect(REPO)
    if os.environ.get("UPDATE_SNAPSHOTS"):
        LIST.write_text("\n".join(found) + "\n", encoding="utf-8")
    assert wording_keys() == found, "run with UPDATE_SNAPSHOTS=1 to regenerate wording_keys.txt"
    assert sum(k.startswith("harness.") for k in found) >= 40
    assert sum(k.startswith("tool.") for k in found) == 16


def test_missing_figure_counts_unworded_keys():
    from types import SimpleNamespace

    from dramatis_forge.report.figures import _d_wording_missing

    keys = wording_keys()
    value, _shown = _d_wording_missing(SimpleNamespace(pack=SimpleNamespace(wording={})), None)
    assert value == len(keys)
    full = dict.fromkeys(keys, "x")
    assert _d_wording_missing(SimpleNamespace(pack=SimpleNamespace(wording=full)), None)[0] == 0
