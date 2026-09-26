"""Normalisation: mechanism for turning wikitext into records, plus the guards.

`wikitext` flattens markup under a pack's rules, `records` fixes the vocabulary,
`guards` turns assumptions into assertions, `runner` orchestrates. No module here
names a template, a character, or a site.

`runner` is imported on demand rather than re-exported: it depends on the store,
which depends on this package's record vocabulary, and eagerly importing it here
would close that loop.
"""

from .guards import GUARDS, Finding, Ledger, Reconciliation
from .records import BY_KIND, KINDS, ORDER, PARSER_VERSION, Record
from .wikitext import Cleaner

__all__ = [
    "BY_KIND", "GUARDS", "KINDS", "ORDER", "PARSER_VERSION",
    "Cleaner", "Finding", "Ledger", "Reconciliation", "Record",
]
