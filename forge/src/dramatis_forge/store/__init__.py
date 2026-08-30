"""Two artifacts, two stores.

`archive` is the forge's working truth: normalised records plus an attached cache
of raw markup. `folio` is what ships: retrieval units, vectors, roster, aliases,
prompts, manifest. They are separate files because they have different audiences,
different lifetimes, and different licences.
"""

from .archive import Archive
from .folio import FOLIO_FORMAT_VERSION, Folio

__all__ = ["Archive", "Folio", "FOLIO_FORMAT_VERSION"]
