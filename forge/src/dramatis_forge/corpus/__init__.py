"""Records → retrieval units → a folio.

`chunk` shapes the units, `tokenize` segments them for the lexical path, `build`
assembles the file and runs guard G5. Encoding is a separate stage: it is the only
part that needs model weights, and the chunk set is the part worth iterating on.
"""

from .build import CorpusReport, run
from .chunk import Chunk, build as build_chunks

__all__ = ["Chunk", "CorpusReport", "build_chunks", "run"]
