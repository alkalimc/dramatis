"""Reports for humans: coverage, per-page inspection, attribution, figure checking.

None of these feed the runtime. They exist so a person can answer four questions:
what is in the corpus, did a page parse correctly, where did each record come from,
and do the documents describing all of that still tell the truth.
"""

from . import attribution, coverage, figures, inspect

__all__ = ["attribution", "coverage", "figures", "inspect"]
