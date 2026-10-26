"""Who is speaking: speaker labels to people, from relations the site declares.

A script names its speakers by whatever the scene calls them: a page title, a given name
without the family name, a rank, a disguise, or a placeholder for someone not yet revealed.
Only the first is a page. Matching on page titles alone attributes well under half of the
named lines, and leaves some of the most-quoted people with no scene at all.

Three signals are used, in this order, and every one is a relation the site writes down:

1. **The label is a page title** (or a form's title). It wins over everything: the label is
   what the script says, and a portrait on screen is often another character's (two people
   share the frame, only one speaks).
2. **A redirect or a real-name entry** that points at exactly one person: the site says this
   name is that person's. It outranks the portrait because it is a statement about the
   name, where a portrait is only who was on screen; where the two disagree the portrait
   was another character's.
3. **The portrait shown for the speaker** names a person's operator page by id. Applied to
   the label across the whole corpus when enough lines agree. A label that is a
   placeholder (a name withheld on purpose) is nobody's: the same placeholder is a
   different person each time, and a scene's most-shown portrait is as often the person
   being spoken to. Any other label the three signals leave open stays open.

A pack may also declare an exception for one scene and label (a disguise the script uses
for a known person); it is checked first and nothing else overrides it.

Name similarity is never used. Where a page-title label and the portraits disagree, the
label stands and the disagreement is recorded, so a person can read the cases.
"""

from __future__ import annotations

from collections import Counter, defaultdict
from dataclasses import dataclass, field

from .archive import Archive
from .pack import Pack
from .params import Speaker


@dataclass(frozen=True, slots=True)
class Conflict:
    """A scene in which a page-title label is shown with another person's portrait."""

    scene: str
    label: str
    person: str
    shown: str
    lines: int


@dataclass
class Speakers:
    archive: Archive
    pack: Pack
    params: Speaker
    person_of: dict[str, str] = field(init=False, default_factory=dict)
    #: portrait key -> person, for keys that name exactly one person's page.
    portrait_person: dict[str, str] = field(init=False, default_factory=dict)
    #: loose portrait key -> person, for the looser key (see `IdentityRules.portrait_loose`)
    #: that names exactly one person; a fallback for labels that are not page titles.
    loose_person: dict[str, str] = field(init=False, default_factory=dict)
    #: (scene, label) -> person, from the pack's declared exceptions.
    declared: dict[tuple[str, str], str] = field(init=False, default_factory=dict)
    #: Declared exceptions whose person is on no roster page (the pack went stale).
    stale_declared: list[tuple[str, str, str]] = field(init=False, default_factory=list)
    #: label -> person, for labels that are not page titles and whose own portraits agree.
    label_person: dict[str, str] = field(init=False, default_factory=dict)
    #: label -> person, from redirects and real names that name one person only.
    alias_person: dict[str, str] = field(init=False, default_factory=dict)
    conflicts: dict[tuple[str, str], Conflict] = field(init=False, default_factory=dict)

    def __post_init__(self) -> None:
        self.person_of = self.archive.person_of()
        self.generic = frozenset(self.pack.chunking.generic_speakers)
        self._portraits()
        self._declared()
        self._aliases()
        self._labels()

    # ---- the three tables ----

    def _portraits(self) -> None:
        key_of = self.pack.identity.portrait_key
        if key_of is None:
            return
        seen: dict[str, set[str]] = defaultdict(set)
        for page, person in self.person_of.items():
            row = self.archive.page(page)
            key = key_of(row["wikitext"] or "") if row is not None else None
            if key:
                seen[key].add(person)
        self.portrait_person = {k: next(iter(v)) for k, v in seen.items() if len(v) == 1}
        loose_of = self.pack.identity.portrait_loose
        if loose_of is not None:
            loose: dict[str, set[str]] = defaultdict(set)
            for key, people in seen.items():
                if (lk := loose_of(key)) is not None:
                    loose[lk] |= people
            self.loose_person = {k: next(iter(v)) for k, v in loose.items() if len(v) == 1}

    def _declared(self) -> None:
        for scene, label, person in self.pack.chunking.speaker_as:
            resolved = self.person_of.get(person)
            if resolved is None:
                self.stale_declared.append((scene, label, person))
            else:
                self.declared[(scene, label)] = resolved

    def _portrait_person(self, key: str) -> str | None:
        """The person a portrait key shows, by the exact key or else by the loose one."""
        if key in self.portrait_person:
            return self.portrait_person[key]
        loose_of = self.pack.identity.portrait_loose
        lk = loose_of(key) if loose_of is not None else None
        return self.loose_person.get(lk) if lk is not None else None

    def _aliases(self) -> None:
        roster = set(self.person_of.values())
        seen: dict[str, set[str]] = defaultdict(set)
        for r in self.archive.db.execute(
            "SELECT alias,target FROM aliases WHERE kind IN ('realname','redirect')"
        ):
            person = self.person_of.get(r["target"], r["target"])
            if person in roster:
                seen[r["alias"]].add(person)
        self.alias_person = {a: next(iter(p)) for a, p in seen.items() if len(p) == 1}

    def _labels(self) -> None:
        by_label: dict[str, Counter] = defaultdict(Counter)
        for r in self.archive.db.execute(
            "SELECT speaker,portrait,COUNT(*) AS n FROM lines "
            "WHERE speaker IS NOT NULL AND portrait IS NOT NULL GROUP BY speaker,portrait"
        ):
            by_label[r["speaker"]][r["portrait"]] = r["n"]
        for label, keys in by_label.items():
            if label in self.person_of or label in self.generic or label in self.alias_person:
                continue
            people = Counter()
            for key, n in keys.items():
                if (who := self._portrait_person(key)) is not None:
                    people[who] += n
            total = sum(keys.values())
            if total < self.params.portrait_lines or not people:
                continue
            person, n = people.most_common(1)[0]
            if n / total >= self.params.portrait_share:
                self.label_person[label] = person

    # ---- attribution ----

    def _shown(self, portraits: list[str | None]) -> tuple[str | None, int, int]:
        """The person most of these portraits show, how many did, and how many were shown."""
        keys = [k for k in portraits if k]
        votes = Counter(self.portrait_person[k] for k in keys if k in self.portrait_person)
        if not votes:
            return None, 0, len(keys)
        person, n = votes.most_common(1)[0]
        return person, n, len(keys)

    def owner(self, label: str, scene: str = "") -> str | None:
        """The person a label stands for in one scene, or None.

        A declared exception comes first: a page-title label that is really a disguise is
        exactly what an exception is for.
        """
        if (scene, label) in self.declared:
            return self.declared[(scene, label)]
        if label in self.person_of:
            return self.person_of[label]
        if label in self.generic:
            return None
        return self.alias_person.get(label) or self.label_person.get(label)

    def scene_owners(self, scene: str, rows: list) -> dict[str, str | None]:
        """Label -> person for every speaker of one scene (`rows` carry `speaker`, `portrait`)."""
        by_label: dict[str, list[str | None]] = defaultdict(list)
        for r in rows:
            if r["speaker"]:
                by_label[r["speaker"]].append(r["portrait"])
        out: dict[str, str | None] = {}
        for label, portraits in by_label.items():
            out[label] = self.owner(label, scene)
            own = self.person_of.get(label)
            if own is not None:
                shown, n, total = self._shown(portraits)
                if shown is not None and shown != own and n == total and n >= 3:
                    self.conflicts[(scene, label)] = Conflict(scene, label, own, shown, n)
        return out

    def own_chars(self) -> dict[str, int]:
        """Characters of dialogue each person speaks, over every scene."""
        out: dict[str, int] = defaultdict(int)
        scene, rows = None, []

        def flush() -> None:
            owners = self.scene_owners(scene, rows) if rows else {}
            for r in rows:
                person = owners.get(r["speaker"])
                if person is not None:
                    out[person] += len(r["text"])

        for r in self.archive.db.execute(
            "SELECT scene,speaker,text,portrait FROM lines WHERE speaker IS NOT NULL "
            "ORDER BY scene,seq"
        ):
            if r["scene"] != scene:
                flush()
                scene, rows = r["scene"], []
            rows.append(r)
        flush()
        return dict(out)

    def speaker_aliases(self) -> list[tuple[str, str]]:
        """(label, person) for labels settled by portrait and not already an alias."""
        return sorted((label, person) for label, person in self.label_person.items()
                      if self.alias_person.get(label) != person)


    def placeholders_resolved(self) -> list[str]:
        """Placeholder labels that some table nevertheless names a person for."""
        return sorted(l for l in self.generic
                      if l in self.person_of or l in self.alias_person or l in self.label_person)
