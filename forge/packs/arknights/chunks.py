"""Five retrieval-unit shapes, and why they are five rather than one.

The corpus mixes four kinds of content whose semantic density differs by roughly an
order of magnitude: continuous prose, dialogue script, attribute records, and index
rows. A single character-window splitter over all of them produces units that are
too long for a voice line and too short for an encyclopaedia section, and pools them
into one vector space where a query for an attribute competes with a query for a
scene. Typing the units is what lets each kind be sized on its own terms — and it is
what lets every metric be reported per bucket, without which the dialogue material
(about nine tenths of the text) simply *is* the score.

**Dialogue units do not overlap, and that is a reversal.** An earlier design used a
sliding window of twelve lines with stride six, on the reasoning that an exchange
straddling a boundary would otherwise be unreachable. Measured, that reasoning does
not survive:

  * it produced 67,927 units for 35,196 windows' worth of content — 95% positional
    redundancy, a 1.9x multiplier on the vector store, and a top-k that could be
    three passages each shown twice;
  * the straddling problem it solved is better solved at query time. A hit's
    neighbours are one indexed lookup away (`span_of` + `span_from`), so the reader
    can widen a result *after* ranking it, paying for context only on the handful of
    units actually returned rather than on all of them;
  * a half-window of preceding context is arbitrary anyway. Neighbour expansion gives
    the true adjacent text.

So the corpus stores each line once, and the reader widens on demand. What replaces
stride is a **boundary rule**: aim for `target` lines, then extend up to `max_span`
until the current speaker stops talking. Measured on 400 scenes, 96% of boundaries
land at a genuine speaker change and unit length stays tight (p50 12, p90 16). Cutting
mid-turn is what actually harms a dialogue unit, and that is now rare rather than
guaranteed at every second boundary.

Net: **26,738 dialogue units instead of 67,927**, ~72 MB of vectors instead of ~138 MB,
no span-merge stage in the ranker, and cleaner units.
"""

from __future__ import annotations

from dramatis_forge.pack import ChunkPolicy, ChunkTemplate

#: Task instructions for the encoder's task-conditioned embedding. One per template,
#: because the query distributions genuinely differ: nobody asks an attribute
#: question the way they ask "what happened in that chapter".
TASKS = {
    "lore": "检索泰拉世界的设定、势力、地理与历史",
    "dialogue": "检索剧情中的对话与场景",
    "voice": "检索干员说过的具体台词",
    "profile": "检索某个人物的身份、履历与关系",
    "letter": "检索干员写给博士的信件",
}

TEMPLATES: tuple[ChunkTemplate, ...] = (
    # Encyclopaedia sections arrive pre-sized by their own headings; splitting is only
    # needed for the occasional long one.
    ChunkTemplate("lore", sources=("lore",), task=TASKS["lore"], max_chars=1200),
    # Dialogue: aim for twelve lines, but break where a speaker finishes talking.
    ChunkTemplate("dialogue", sources=("line", "choice"), task=TASKS["dialogue"],
                  max_chars=1400, target=12, max_span=18),
    # One voice line per unit. Short — sixty characters or so with its header — but
    # this is exactly the "what did she say about X" query, and merging lines from
    # unrelated triggers into one unit would make it answer neither.
    ChunkTemplate("voice", sources=("voice",), task=TASKS["voice"], max_chars=400),
    # A person's identity card plus one unit per dossier section, plus non-roster
    # character descriptions.
    ChunkTemplate("profile", sources=("dossier", "char_ref"), task=TASKS["profile"],
                  max_chars=900),
    ChunkTemplate("letter", sources=("letter",), task=TASKS["letter"], max_chars=1400),
)

#: Prepended to the embedded text. A dialogue window with no scene and no speakers is
#: close to unsearchable; a voice line without its trigger is a floating quotation.
HEADERS = {
    "lore": "【设定】{page}{path}",
    "dialogue": "【剧情】{group} · {scene}{speakers}",
    "voice": "【语音】{person} · {trigger}{title}",
    "profile": "【档案】{person}{section}",
    "letter": "【信件】{sender} → 博士{title}",
}

POLICY = ChunkPolicy(templates=TEMPLATES, headers=HEADERS)
