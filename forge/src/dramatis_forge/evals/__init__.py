"""Benchmark construction and evaluation.

`structural` needs no annotation budget and no model: it reads relations the wiki's
editors already wrote. `synthetic` and `adversarial` need a model and are built on top
of it. All three are published separately from the product — the runtime never reads
them, and a benchmark should not be buried inside the data file it measures.
"""

from . import structural

__all__ = ["structural"]
