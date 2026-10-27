"""The `persona.*` parameter group.

Field names mirror the calibration register (`persona.prompt_tokens` is
`Persona.prompt_tokens`). The defaults are the register's current values and this is the
only place they are written; use sites take the struct as an argument. Overrides come
from the `[persona]` table of an optional `params.toml`; missing keys keep their defaults.
"""

from __future__ import annotations

import tomllib
from dataclasses import dataclass, fields, replace
from pathlib import Path


@dataclass(frozen=True)
class Persona:
    #: Upper bound on the generated `system_prompt` (and each host layer), in tokens of
    #: the pack's reference tokenizer, or of the documented estimate when it is absent.
    prompt_tokens: int = 400
    #: Upper bound on `capability_line`, in characters.
    capability_chars: int = 40
    #: Inclusive bounds on the number of `tone_rules`.
    tone_rules: tuple[int, int] = (3, 4)
    #: Input budget for one person's whole material block, in tokens. Facts reach the
    #: speaking model by retrieval, so the brief needs only enough of a person's own
    #: words and a few identity lines to hear how they speak.
    material_tokens: int = 5000
    #: Share of `material_tokens` reserved for the sample of story lines, in tokens.
    story_tokens: int = 1500
    #: The same two budgets when the persona model is billed per request, whatever the
    #: size of the call (`billing = "request"` in the endpoints file): one call carries
    #: far more of a person's material than a sample. Still bounded: a request-billed proxy
    #: may cap the request in bytes (the local Gemini one takes 128,000). A request runs
    #: about 10 kB plus 5 bytes per counted token, so 10000 lands near 60 kB: half the cap.
    request_material_tokens: int = 10000
    request_story_tokens: int = 5000
    #: Own lines of a person kept as tone references: how many, the budget for all of
    #: them together (a line may take at most a quarter of it), and the shortest line
    #: worth keeping, in tokens. They are picked from the material, not generated.
    example_lines: int = 8
    example_tokens: int = 400
    example_min_tokens: int = 8
    #: Attempts per endpoint call before the person is reported as failed.
    retries: int = 3
    #: Total seconds one call may spend waiting out HTTP 429 (a requests-per-hour cap on
    #: a proxy, say) before the person is reported as an error. Waits are not attempts.
    #: An hour is one full window of such a cap.
    rate_limit_wait_s: float = 3600.0
    #: Least number of voice and dialogue units attributed to a person for them to get a
    #: persona at all. Below it they have no words of their own, only a card or a note
    #: about them, and a generator can only invent a voice. 0 turns the rule off.
    min_own_units: int = 1

    def prompt_values(self) -> dict[str, int]:
        """The budgets a generator prompt may state, by placeholder name. `prompt_chars`
        is a character bound the model can count while writing; it is the token bound in
        wide characters, which the estimate in `material` counts one-for-one."""
        return {"prompt_tokens": self.prompt_tokens, "prompt_chars": self.prompt_tokens,
                "capability_chars": self.capability_chars,
                "tone_min": self.tone_rules[0], "tone_max": self.tone_rules[1]}

    def billed(self, per_request: bool) -> Persona:
        """The parameters a run uses: the larger input budgets under per-request billing."""
        if not per_request:
            return self
        return replace(self, material_tokens=self.request_material_tokens,
                       story_tokens=self.request_story_tokens)

    @classmethod
    def load(cls, path: Path | None) -> Persona:
        if path is None or not path.exists():
            return cls()
        given = tomllib.loads(path.read_text(encoding="utf-8")).get("persona")
        if not isinstance(given, dict):
            return cls()
        known = {f.name for f in fields(cls)}
        values = {k: v for k, v in given.items() if k in known}
        if "tone_rules" in values:
            lo, hi = values["tone_rules"]
            values["tone_rules"] = (int(lo), int(hi))
        return replace(cls(), **values)
