"""The three write gates. A person who fails any of them is not written.

② meta leak: meta-layer vocabulary in any generated text a non-host session will carry
  (for the host: everything but its `meta` slot). The only irreparable breach, since
  other persons' sessions must never contain the host's meta vocabulary.
⑤ structure and budget: schema complete, `system_prompt` within `persona.prompt_tokens`,
  `capability_line` within `persona.capability_chars`, `tone_rules` count within
  `persona.tone_rules`, `fallback_line` non-empty.
⑥ prescriptive phrasing: capability claims and negative knowledge lists in
  `system_prompt` and `tone_rules` (the host's `in_world` too, which sits beside them).
  Those behaviours come from retrieval and visibility, and a claim that disagrees with
  what retrieval returns makes the model invent.

Gates judge; they never repair. A failure is fixed by changing the generator.
"""

from __future__ import annotations

import re
from collections.abc import Callable, Iterable
from dataclasses import dataclass, field

from .generator import Generator
from .params import Persona

META = "meta_leak"
STRUCTURE = "structure"
PRESCRIPTIVE = "prescriptive"


@dataclass(frozen=True)
class Finding:
    gate: str
    slot: str
    detail: str


@dataclass
class Verdict:
    findings: list[Finding] = field(default_factory=list)
    #: Token count of each budgeted slot, as measured.
    tokens: dict[str, int] = field(default_factory=dict)

    @property
    def passed(self) -> bool:
        return not self.findings

    def count(self, gate: str) -> int:
        return sum(1 for f in self.findings if f.gate == gate)

    def reasons(self) -> list[str]:
        return [f"{f.gate} [{f.slot}] {f.detail}" for f in self.findings]


def _term_pattern(term: str) -> re.Pattern[str]:
    # An ASCII word inside a longer ASCII word is a different word ("API" in "RAPID");
    # other scripts have no word boundary to lean on.
    if term.isascii():
        return re.compile(rf"(?<![A-Za-z0-9]){re.escape(term)}(?![A-Za-z0-9])", re.IGNORECASE)
    return re.compile(re.escape(term))


class Lexicons:
    """The generator's lexicons, compiled once per run."""

    def __init__(self, gen: Generator) -> None:
        self.meta = [(t, _term_pattern(t)) for t in gen.meta_terms]
        self.meta += [(p, re.compile(p)) for p in gen.meta_patterns]
        self.prescriptive = [(p, re.compile(p, re.IGNORECASE)) for p in gen.prescriptive_patterns]

    @staticmethod
    def _hits(text: str, patterns: Iterable[tuple[str, re.Pattern[str]]]) -> list[str]:
        return [m.group(0) for _name, rx in patterns for m in rx.finditer(text)]

    def meta_hits(self, text: str) -> list[str]:
        return self._hits(text, self.meta)

    def prescriptive_hits(self, text: str) -> list[str]:
        return self._hits(text, self.prescriptive)


def _texts(output: dict, slot: str) -> list[str]:
    value = output.get(slot)
    if isinstance(value, list):
        return [v for v in value if isinstance(v, str)]
    return [value] if isinstance(value, str) else []


def check(output: dict, *, host: bool, gen: Generator, lex: Lexicons, p: Persona,
          count: Callable[[str], int]) -> Verdict:
    v = Verdict()
    add = v.findings.append

    # ⑤ schema first: the other gates read the fields it vouches for.
    for name in gen.fields(host):
        value = output.get(name)
        if name == "tone_rules":
            ok = isinstance(value, list) and all(isinstance(r, str) and r.strip() for r in value)
        else:
            ok = isinstance(value, str) and bool(value.strip())
        if not ok:
            add(Finding(STRUCTURE, name, "missing, empty or of the wrong type"))
    extra = sorted(set(output) - set(gen.fields(host)))
    if extra:
        add(Finding(STRUCTURE, "*", f"unexpected field(s): {', '.join(extra)}"))

    budgeted = ("system_prompt", "in_world", "meta") if host else ("system_prompt",)
    for name in budgeted:
        text = output.get(name)
        if isinstance(text, str):
            v.tokens[name] = n = count(text)
            if n > p.prompt_tokens:
                add(Finding(STRUCTURE, name, f"{n} tokens > persona.prompt_tokens "
                                             f"{p.prompt_tokens}"))
    cap = output.get("capability_line")
    if isinstance(cap, str) and len(cap.strip()) > p.capability_chars:
        add(Finding(STRUCTURE, "capability_line",
                    f"{len(cap.strip())} chars > persona.capability_chars {p.capability_chars}"))
    rules = output.get("tone_rules")
    lo, hi = p.tone_rules
    if isinstance(rules, list) and not lo <= len(rules) <= hi:
        add(Finding(STRUCTURE, "tone_rules",
                    f"{len(rules)} rules, outside persona.tone_rules {lo}-{hi}"))

    # ② every slot that reaches a session other than the host's own meta layer.
    for name in gen.fields(host):
        if name == "meta":
            continue
        for text in _texts(output, name):
            for hit in lex.meta_hits(text):
                add(Finding(META, name, f"meta-layer term {hit!r}"))

    # ⑥ the prompt proper: what the speaking model is told it is.
    for name in ("system_prompt", "tone_rules") + (("in_world",) if host else ()):
        for text in _texts(output, name):
            for hit in lex.prescriptive_hits(text):
                add(Finding(PRESCRIPTIVE, name, f"prescriptive phrasing {hit!r}"))
    return v
