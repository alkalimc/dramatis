"""Reports for humans: coverage, per-page inspection, sampling, attribution, figures.

None of these feed the runtime. They exist so a person can answer five questions: what
is in the corpus, did a page parse correctly, which pages should be read after a sync,
where did each record come from, and do the documents describing all of that still tell
the truth.
"""

from . import attribution, coverage, figures, inspect, samples

__all__ = ["attribution", "coverage", "figures", "inspect", "samples"]
