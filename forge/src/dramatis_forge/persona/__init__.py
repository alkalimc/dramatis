"""`forge persona`: offline persona synthesis, gated, into the folio's `prompts` table.

The maintainer runs this once per generator version; the application only assembles what
it writes and never generates a persona itself. One call per person:

    material (every form, each item tagged with its form page)
      -> generator prompt (the pack's; a neutral default otherwise)
      -> the `persona` endpoint role, structured output
      -> gates: meta-layer leak, structure and budget, prescriptive phrasing
      -> `prompts` rows, only for a person who passes every gate

What goes into a persona is deliberately narrow: how the person speaks, what they care
about, what they say when unsure, and who they are. Anything the runtime produces from
structure (what they know, how much they find, whether they come to you) must stay out,
and the prescriptive-phrasing gate is what enforces it.
"""

from .generator import Generator
from .params import Persona

__all__ = ["Generator", "Persona"]
