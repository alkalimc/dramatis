"""Where outputs go: the folio's `prompts` rows, and a raw cache beside the folio.

Rows: one per slot, all stamped with the generator version. A subject's slots are
replaced together in one transaction, so a subject is either fully written by one
version or not at all.

The raw cache (`<pack dir>/persona/raw/<subject>.json`) holds each subject's last reply
with its version. `forge build` creates the folio afresh, which drops every prompt row;
the cache lets the next `forge persona` restore rows for the same version without
calling the endpoint again.

The entry also holds the subject's example lines, so a rebuilt folio gets them back too.
Each entry also records what the reply was generated from (`source`: the ids of the units
whose text was sent, and of every unit that could have been), which `stale` compares with
a rebuilt folio.
"""

from __future__ import annotations

import hashlib
import json
import re
import sqlite3
from pathlib import Path
from typing import Any

#: Output field -> prompts slot.
SLOTS = {
    "system_prompt": "system",
    "tone_rules": "tone",
    "capability_line": "capability",
    "in_world": "in_world",
    "meta": "meta",
}
FIELDS = {slot: name for name, slot in SLOTS.items()}
#: Not a generated field: the person's own lines, picked from the material (see
#: `material.select_examples`) and stored with the subject's other rows, one per line.
LINES = "lines"


def tone_body(rules: list[str]) -> str:
    """One rule per line; a rule's own line breaks are folded so the split is exact."""
    return "\n".join(" ".join(r.split()) for r in rules)


def to_rows(output: dict[str, Any], fields: tuple[str, ...]) -> list[tuple[str, str]]:
    rows = []
    for name in fields:
        value = output[name]
        rows.append((SLOTS[name], tone_body(value) if name == "tone_rules" else value.strip()))
    return rows


def read(db: sqlite3.Connection, subject: str,
         fields: tuple[str, ...]) -> tuple[str, dict[str, Any]] | None:
    """(version, output) of a subject's stored rows, or None unless every slot is there
    and all of them carry one version."""
    rows = {slot: (body, version) for slot, body, version in db.execute(
        "SELECT slot, body, generator_version FROM prompts WHERE subject=?", (subject,))}
    wanted = [SLOTS[name] for name in fields]
    if not all(slot in rows for slot in wanted):
        return None
    versions = {rows[slot][1] for slot in wanted}
    if len(versions) != 1:
        return None
    output: dict[str, Any] = {}
    for slot in wanted:
        body = rows[slot][0]
        output[FIELDS[slot]] = [ln for ln in body.split("\n") if ln] if slot == "tone" else body
    return versions.pop(), output


def read_lines(db: sqlite3.Connection, subject: str) -> list[str]:
    row = db.execute("SELECT body FROM prompts WHERE subject=? AND slot=?",
                     (subject, LINES)).fetchone()
    return [ln for ln in row[0].split("\n") if ln] if row else []


def write(db: sqlite3.Connection, subject: str, output: dict[str, Any],
          fields: tuple[str, ...], version: str, lines: list[str] | None = None) -> None:
    rows = to_rows(output, fields)
    if lines:
        rows.append((LINES, "\n".join(lines)))
    slots = [slot for slot, _ in rows] + ([] if lines else [LINES])
    with db:
        db.execute(f"DELETE FROM prompts WHERE subject=? AND slot IN ({','.join('?' * len(slots))})",
                   (subject, *slots))
        db.executemany(
            "INSERT INTO prompts(subject, slot, body, generator_version) VALUES(?,?,?,?)",
            [(subject, slot, body, version) for slot, body in rows])


def remove(db: sqlite3.Connection, subject: str, fields: tuple[str, ...]) -> None:
    """Drop a subject's rows: a person whose current output fails is shown as missing,
    not as an older persona."""
    slots = [SLOTS[name] for name in fields] + [LINES]
    with db:
        db.execute(f"DELETE FROM prompts WHERE subject=? AND slot IN ({','.join('?' * len(slots))})",
                   (subject, *slots))


def restamp(db: sqlite3.Connection, subject: str, old: str, new: str) -> int:
    """Move a subject's rows of version `old` to version `new`; how many moved."""
    with db:
        return db.execute("UPDATE prompts SET generator_version=? "
                          "WHERE subject=? AND generator_version=?", (new, subject, old)).rowcount


def safe_name(subject: str) -> str:
    """A file name for any subject: readable where possible, unique always."""
    stem = re.sub(r'[\\/:*?"<>|\s]+', "_", subject).strip("._") or "subject"
    digest = hashlib.sha256(subject.encode()).hexdigest()[:8]
    return f"{stem[:60]}-{digest}"


class RawCache:
    def __init__(self, root: Path) -> None:
        self.root = root

    def path(self, subject: str) -> Path:
        return self.root / f"{safe_name(subject)}.json"

    def load(self, subject: str, version: str) -> dict[str, Any] | None:
        path = self.path(subject)
        if not path.exists():
            return None
        try:
            entry = json.loads(path.read_text(encoding="utf-8"))
        except json.JSONDecodeError:
            return None
        if entry.get("subject") != subject or entry.get("generator_version") != version:
            return None
        output = entry.get("output")
        return output if isinstance(output, dict) else None

    def lines(self, subject: str, version: str) -> list[str]:
        """The example lines saved with the reply of this version."""
        path = self.path(subject)
        if not path.exists():
            return []
        try:
            entry = json.loads(path.read_text(encoding="utf-8"))
        except json.JSONDecodeError:
            return []
        lines = entry.get("lines")
        if entry.get("generator_version") != version or not isinstance(lines, list):
            return []
        return [ln for ln in lines if isinstance(ln, str)]

    def entry(self, subject: str) -> dict[str, Any] | None:
        """The stored entry of any version, or None."""
        path = self.path(subject)
        if not path.exists():
            return None
        try:
            entry = json.loads(path.read_text(encoding="utf-8"))
        except json.JSONDecodeError:
            return None
        return entry if isinstance(entry, dict) and entry.get("subject") == subject else None

    def restamp(self, subject: str, old: str, new: str) -> bool:
        """Give a stored reply of version `old` the version `new`, keeping everything else."""
        entry = self.entry(subject)
        if entry is None or entry.get("generator_version") != old:
            return False
        path = self.path(subject)
        tmp = path.with_suffix(".tmp")
        tmp.write_text(json.dumps({**entry, "generator_version": new},
                                  ensure_ascii=False, indent=2), encoding="utf-8")
        tmp.replace(path)
        return True

    def save(self, subject: str, version: str, output: dict[str, Any],
             raw: dict[str, Any], usage: dict[str, int],
             source: dict[str, list[str]] | None = None,
             lines: list[str] | None = None) -> Path:
        self.root.mkdir(parents=True, exist_ok=True)
        path = self.path(subject)
        tmp = path.with_suffix(".tmp")
        tmp.write_text(json.dumps({"subject": subject, "generator_version": version,
                                   "source": source, "lines": lines or [], "output": output,
                                   "usage": usage,
                                   "response": raw},
                                  ensure_ascii=False, indent=2), encoding="utf-8")
        tmp.replace(path)
        return path
