"""The generator: prompts, output schema, lexicons, and the version that names them.

All of it is pack content. A pack supplies `GENERATOR` in a `persona` module next to its
`PACK` (`packs/<name>/persona.py`); without one, the neutral English default below is
used. The default exists so the mechanism runs end to end on any pack; a real corpus
wants a generator written in its own language, for its own fiction.

`version` is derived, never typed: a digest of everything that changes what the model is
asked (prompts, schema, material budget, model, reasoning). Editing one word of a prompt
therefore gives a new version, and a resumed run cannot mistake an old output for a
current one.
"""

from __future__ import annotations

import hashlib
import importlib
import json
from collections.abc import Mapping
from dataclasses import dataclass, field
from types import ModuleType
from typing import Any

from ..pack import _SEARCH, Pack
from .params import Persona

#: Bump when this module changes how prompts are assembled or outputs are read.
CODE_VERSION = "1"

#: Output fields every subject returns, in order.
PERSON_FIELDS = ("system_prompt", "tone_rules", "fallback_line", "capability_line")
#: Output fields the host returns on top of `PERSON_FIELDS`.
HOST_FIELDS = ("in_world", "meta")

#: Placeholders a user template may use; each is replaced literally, so a template may
#: contain any other braces (a JSON example, say) without escaping.
PLACEHOLDERS = ("name", "forms", "material", "prompt_tokens", "prompt_chars",
                "capability_chars", "tone_min", "tone_max", "user_rule", "user_placeholder")


@dataclass(frozen=True)
class HostSource:
    """Where the host's own material is. The host is not a roster person, so it has no
    attribution rows; its material is found by name instead.

    `names`: speaker names of its lines in dialogue units. `titles`: exact unit titles
    (or the last breadcrumb segment of one) whose units describe it.
    """

    names: tuple[str, ...]
    titles: tuple[str, ...] = ()


@dataclass(frozen=True)
class Generator:
    #: Human label prefixed to the derived version.
    label: str
    system: str
    user: str
    #: The host's prompts. Empty `host_system` means the host is not generated.
    host_system: str = ""
    host_user: str = ""
    host: HostSource | None = None
    #: Meta-layer vocabulary: words that must never appear outside the host's meta layer.
    #: ASCII words match on word boundaries, case-insensitively; others as substrings.
    meta_terms: tuple[str, ...] = ()
    #: Regular expressions for meta-layer phrasing a term list cannot express.
    meta_patterns: tuple[str, ...] = ()
    #: Regular expressions for capability claims and negative knowledge lists.
    prescriptive_patterns: tuple[str, ...] = ()
    #: Quotation marks (open, close) around an example of the person's own words. The
    #: prescriptive gate skips quoted spans, since "never bring that up", quoted, is
    #: something they say rather than something they are told; the meta gate does not.
    quotes: tuple[tuple[str, str], ...] = (("\u201c", "\u201d"), ('"', '"'))
    #: Section headings of the material block, keyed by material kind (see `material`).
    labels: Mapping[str, str] = field(default_factory=dict)
    #: Reclassify units by title: (from kind, regular expression matched in full against
    #: the unit title, to kind), first match wins; to kind `drop` removes the unit. The
    #: folio tells a dossier section from an archive quote only by title, and some units
    #: (an item description, a menu caption) are about something other than the person.
    title_kinds: tuple[tuple[str, str, str], ...] = ()
    #: Regular expressions matched in full against each line of a non-dialogue unit;
    #: matching lines are left out of the material (a stat row on an identity card, a
    #: provenance note). What the model never sees it cannot repeat.
    line_drops: tuple[str, ...] = ()
    #: The prompt line about addressing the user, filled into `{user_rule}` when the corpus
    #: declares a user-name placeholder (`{user_placeholder}` is replaced by it). Material
    #: and output keep the placeholder verbatim; only runtime assembly substitutes it.
    user_rule: str = ""
    #: Regular expressions for other names for the user in generated text (a made-up
    #: name, a title followed by something other than the placeholder).
    user_name_patterns: tuple[str, ...] = ()

    def label_for(self, kind: str) -> str:
        return self.labels.get(kind) or DEFAULT_LABELS.get(kind, kind)

    def fields(self, host: bool) -> tuple[str, ...]:
        return PERSON_FIELDS + (HOST_FIELDS if host else ())

    def schema(self, host: bool) -> dict[str, Any]:
        """JSON schema of one output, strict-mode compatible (every property required, no
        extras). Counts and lengths are the gates' job: not every endpoint enforces them."""
        props: dict[str, Any] = {name: {"type": "string"} for name in self.fields(host)}
        props["tone_rules"] = {"type": "array", "items": {"type": "string"}}
        return {"type": "object", "properties": props, "required": list(self.fields(host)),
                "additionalProperties": False}

    def user_line(self, placeholder: str) -> str:
        return fill(self.user_rule, {"user_placeholder": placeholder}) if placeholder else ""

    def version(self, *, model: str, reasoning: str | None, params: Persona,
                placeholder: str = "") -> str:
        """Digest of everything that changes what the model is asked. Gate lexicons are
        left out on purpose: a resumed run re-checks stored rows against the current
        lexicons, so tightening a gate costs no regeneration for rows that still pass."""
        h = hashlib.sha256()
        for part in (CODE_VERSION, self.system, self.user, self.host_system, self.host_user,
                     json.dumps(self.schema(True), sort_keys=True),
                     json.dumps(dict(self.labels), sort_keys=True, ensure_ascii=False),
                     repr(self.title_kinds), repr(self.line_drops), self.user_rule,
                     placeholder, repr(self.host),
                     model, reasoning or "", repr(params.prompt_values()),
                     str(params.material_tokens), str(params.story_tokens)):
            h.update(part.encode())
            h.update(b"\x00")
        return f"{self.label}-{h.hexdigest()[:12]}"


def fill(template: str, values: Mapping[str, object]) -> str:
    out = template
    for key in PLACEHOLDERS:
        if key in values:
            out = out.replace("{" + key + "}", str(values[key]))
    return out


DEFAULT_LABELS: Mapping[str, str] = {
    "card": "Identity card",
    "dossier": "Dossier",
    "quote": "Archive quotes",
    "char_ref": "Character notes",
    "voice": "Voice lines (text)",
    "letter": "Letters",
    "lore": "Encyclopaedia",
    "story": "Story lines (sample)",
}

_DEFAULT_SYSTEM = """\
You write the opening character brief for one person in an interactive fiction. The brief
is placed, unchanged, at the start of every conversation with that person; another model
reads it and speaks as them. Use only the source material you are given.

The brief covers exactly four things:
1. How they speak: tone, sentence length and rhythm, verbal habits, how they address others.
2. What they care about: loyalties, convictions, what they avoid or dislike.
3. What they say when unsure: one line in their own voice.
4. Who they are: identity, role, affiliation, origin, as the material states them.

Never write:
- Claims about capability or talkativeness ("you are erudite", "you know everything",
  "you talk little"). How much they know and say comes from what they can actually look
  up at conversation time, not from the brief.
- Lists of what they do not know or must not mention ("you don't know X", "never bring
  up Y"). What they should not know is simply never shown to them.
- Behaviour imperatives or numbers ("start conversations", "reply in under N words").
- Anything outside their world: the words game, story, script, player, role-play, model,
  prompt, material, wiki. The person does not know they were written.
- Experiences, relations or details the material does not contain. Short beats invented;
  thin material gets a short brief, not adjectives.

About the material: every item is tagged with the page of the form it comes from, in
square brackets. Several forms are one person at different times or in different roles;
say how their speech differs between them if it does. Voice lines are text only: take
wording, address and habits from them, never anything about a voice or its trigger.
Story lines are things they said; an indented line just before one is what it answers
(another speaker, narration, or options the user could have picked, which were not
necessarily said), context only.
{user_rule}

Fields:
- system_prompt: continuous prose in the second person, at most {prompt_chars} characters.
- tone_rules: {tone_min} to {tone_max} one-sentence rules about how they speak, each with a
  short example from the material where possible.
- fallback_line: one line they would say when unsure, as spoken, first person.
- capability_line: identity and field for a roster, at most {capability_chars} characters,
  no praise.
"""

_DEFAULT_USER = """\
Person: {name}
Forms: {forms}

Material:

{material}
"""

_DEFAULT_HOST_SYSTEM = _DEFAULT_SYSTEM + """
This subject is the host: the in-world assistant the user talks to first. It has two
layers of one identity, used together. Also write:
- in_world: its role and outlook inside the fiction, in-world words only.
- meta: how it speaks when the topic is the simulation itself (that the world is
  simulated, which model answers, how much budget is left). Its voice, not a feature list.
Everything except `meta` follows the rule against words from outside the world.
"""

#: English meta-layer vocabulary for the default generator.
DEFAULT_META_TERMS = (
    "language model", "LLM", "AI model", "prompt", "system prompt", "token", "endpoint",
    "quota", "API", "context window", "simulation", "role-play", "roleplay", "player",
    "game", "script",
)

#: English capability claims and negative knowledge lists for the default generator.
DEFAULT_PRESCRIPTIVE = (
    r"\byou(?:'re| are)\s+(?:very\s+|quite\s+|extremely\s+|highly\s+)?"
    r"(?:erudite|knowledgeable|well[- ]read|all[- ]knowing|taciturn|talkative|quiet)\b",
    r"\byou\s+(?:talk|speak|say)\s+(?:very\s+)?(?:little|rarely|a lot|much)\b",
    r"\byou\s+know\s+(?:everything|a lot|all)\b",
    r"\byou\s+(?:don't|do not|never|cannot|can't)\s+(?:know|remember|recall|understand)\b",
    r"\b(?:don't|do not|never|avoid)\s+(?:mention|bring(?:ing)? up|talk(?:ing)? about|"
    r"discuss(?:ing)?|reveal(?:ing)?)\b",
)

DEFAULT = Generator(
    label="default",
    system=_DEFAULT_SYSTEM,
    user=_DEFAULT_USER,
    user_rule=("The material writes the user's name as {user_placeholder}. Wherever the "
               "person addresses the user by name, write {user_placeholder} exactly as it "
               "is; never make up a name for the user."),
    meta_terms=DEFAULT_META_TERMS,
    prescriptive_patterns=DEFAULT_PRESCRIPTIVE,
)


def _module(pack_name: str) -> ModuleType | None:
    for pattern in _SEARCH:
        name = pattern.format(name=pack_name) + ".persona"
        try:
            return importlib.import_module(name)
        except ModuleNotFoundError as exc:
            if exc.name is not None and not name.startswith(exc.name):
                raise  # a real import error inside the pack's module; do not mask it
    return None


def load(pack: Pack) -> Generator:
    """The pack's `persona.GENERATOR`, else the neutral default."""
    mod = _module(pack.name)
    if mod is None:
        return DEFAULT
    gen = getattr(mod, "GENERATOR", None)
    if not isinstance(gen, Generator):
        raise TypeError(f"{mod.__name__} defines no module-level GENERATOR: Generator")
    return gen
