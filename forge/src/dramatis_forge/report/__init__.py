"""Reports for humans: coverage, per-page inspection, attribution.

None of these feed the runtime. They exist so a person can answer three questions:
what is in the corpus, did a page parse correctly, and where did each record come from.
"""

from . import attribution, coverage, inspect

__all__ = ["attribution", "coverage", "inspect"]
