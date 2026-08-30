"""Harvest: the three stages that talk to the wiki.

`scope` enumerates what exists, `fetch` retrieves it once, `update` keeps it
current from the site's change feed. Nothing downstream of `harvest` makes a
network request.
"""

from . import fetch, scope, update

__all__ = ["fetch", "scope", "update"]
