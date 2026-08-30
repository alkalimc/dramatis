"""Page readers for the Arknights pack.

One module per family of pages, and inside `index.py` one function per hand-listed
index page. Each reader's docstring records what its page is and what was learnt from
running it against the whole thing — those notes are the pack's real documentation,
because every one of them corresponds to a rule that was wrong first.
"""

from .dossier import parse_dossier
from .index import (
    parse_char_refs,
    parse_glossary,
    parse_letters,
    parse_prose,
    parse_real_names,
    parse_synopses,
    parse_tips,
)
from .lore import parse_lore
from .voice import parse_voices

__all__ = [
    "parse_char_refs", "parse_dossier", "parse_glossary", "parse_letters", "parse_lore",
    "parse_prose", "parse_real_names", "parse_synopses", "parse_tips", "parse_voices",
]
