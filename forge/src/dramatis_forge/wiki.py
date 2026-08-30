"""A polite MediaWiki client.

We are a guest on someone else's volunteer-funded server, making thousands of
requests. The politeness is not decoration:

  * token-bucket rate limit, one knob, default well under any plausible limit
  * `maxlag` so we step aside when the site's replicas fall behind
  * exponential backoff with jitter on transport errors
  * a User-Agent naming the project and a way to reach a human

Two batch limits exist and must not be conflated: `list=` accepts 500 per
request, `titles=` accepts 50. Using 500 with `titles=` gets a truncated
response plus a warning that is easy to miss.
"""

from __future__ import annotations

import time
from collections.abc import Iterable, Iterator
from typing import Any

import httpx
from tenacity import retry, retry_if_exception_type, stop_after_attempt, wait_exponential_jitter

LIST_LIMIT = 500
TITLES_LIMIT = 50


class WikiError(RuntimeError):
    """An error the API reported in-band, i.e. with HTTP 200 and `error` set."""


class Throttled(WikiError):
    """`maxlag` — the site asked us to wait. Always worth retrying."""


class Wiki:
    def __init__(
        self,
        api: str,
        contact: str,
        *,
        rate: float = 8.0,
        timeout: float = 120.0,
        user_agent: str | None = None,
    ) -> None:
        self.api = api
        self._min_interval = 1.0 / max(rate, 0.01)
        self._last = 0.0
        self.requests = 0
        ua = user_agent or f"dramatis-forge (+{contact})"
        self._c = httpx.Client(
            headers={"User-Agent": ua, "Accept-Encoding": "gzip"},
            timeout=timeout,
            follow_redirects=False,
        )

    # ---- lifecycle ----

    def close(self) -> None:
        self._c.close()

    def __enter__(self) -> Wiki:
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    # ---- transport ----

    def _throttle(self) -> None:
        gap = time.monotonic() - self._last
        if gap < self._min_interval:
            time.sleep(self._min_interval - gap)
        self._last = time.monotonic()

    @retry(
        retry=retry_if_exception_type((Throttled, httpx.TransportError, httpx.HTTPStatusError)),
        stop=stop_after_attempt(6),
        wait=wait_exponential_jitter(initial=1, max=60),
        reraise=True,
    )
    def get(self, **params: Any) -> dict:
        self._throttle()
        params.setdefault("format", "json")
        params.setdefault("formatversion", 2)
        params.setdefault("maxlag", 5)
        self.requests += 1
        r = self._c.get(self.api, params=params)
        r.raise_for_status()
        d = r.json()
        if "error" in d:
            code = d["error"].get("code")
            if code == "maxlag":
                time.sleep(5)
                raise Throttled(d["error"].get("info", "maxlag"))
            raise WikiError(f"{code}: {d['error'].get('info', '')}")
        return d

    def paged(self, **params: Any) -> Iterator[dict]:
        """Follow `continue` to exhaustion, yielding each response's `query`.

        Note the guard against a non-advancing cursor: a malformed continuation
        (or a proxy that strips it) otherwise turns this into an infinite loop
        that hammers the site.
        """
        seen: set[str] = set()
        while True:
            d = self.get(**params)
            if "query" in d:
                yield d["query"]
            cont = d.get("continue")
            if not cont:
                return
            token = repr(sorted(cont.items()))
            if token in seen:
                raise WikiError(f"continuation did not advance: {cont}")
            seen.add(token)
            params.update(cont)

    # ---- enumeration ----

    def allpages(self, ns: int = 0, *, redirects: str | None = None) -> Iterator[str]:
        params: dict[str, Any] = dict(
            action="query", list="allpages", apnamespace=ns, aplimit=LIST_LIMIT
        )
        if redirects:
            params["apfilterredir"] = redirects
        for q in self.paged(**params):
            for p in q.get("allpages", []):
                yield p["title"]

    def embeddedin(self, template: str, ns: int | None = None) -> Iterator[str]:
        """Which pages transclude this template.

        Two orders of magnitude cheaper than asking every page for its template
        list, and — unlike `generator=allpages&prop=templates` — it does not
        terminate early when `batchcomplete` interleaves with `continue`. That
        early termination was measured, not theorised: it silently returned
        complete template lists for only a minority of pages.
        """
        params: dict[str, Any] = dict(
            action="query", list="embeddedin", eititle=template, eilimit=LIST_LIMIT
        )
        if ns is not None:
            params["einamespace"] = ns
        for q in self.paged(**params):
            for p in q.get("embeddedin", []):
                yield p["title"]

    def categorymembers(self, category: str) -> Iterator[str]:
        for q in self.paged(
            action="query", list="categorymembers", cmtitle=category, cmlimit=LIST_LIMIT
        ):
            for p in q.get("categorymembers", []):
                yield p["title"]

    def cargo(self, table: str, fields: str, *, limit: int = LIST_LIMIT) -> list[dict]:
        """Full Cargo table with manual offset paging.

        `_pageName` must be aliased (`_pageName=page`); querying it bare returns
        `cargoquery-invalidfieldalias`, which reads like "the table is
        unavailable" and has been misdiagnosed as exactly that.
        """
        out: list[dict] = []
        offset = 0
        while True:
            d = self.get(
                action="cargoquery", tables=table, fields=fields, limit=limit, offset=offset
            )
            rows = [r["title"] for r in d.get("cargoquery", [])]
            out += rows
            if len(rows) < limit:
                return out
            offset += limit

    # ---- content ----

    def content(self, titles: Iterable[str]) -> Iterator[tuple[str, str | None, int | None]]:
        """Batched wikitext with revision ids.

        The revid is not optional bookkeeping: it is what makes every record in
        the corpus traceable to one version of one page, which is the whole basis
        of the attribution obligation for redistributing the text.
        """
        batch: list[str] = []
        for t in titles:
            batch.append(t)
            if len(batch) == TITLES_LIMIT:
                yield from self._content_batch(batch)
                batch = []
        if batch:
            yield from self._content_batch(batch)

    def _content_batch(self, batch: list[str]) -> Iterator[tuple[str, str | None, int | None]]:
        d = self.get(
            action="query", prop="revisions", rvslots="main",
            rvprop="content|ids", titles="|".join(batch),
        )
        pages = d.get("query", {}).get("pages", [])
        seen: set[str] = set()
        for p in pages:
            title = p.get("title", "?")
            seen.add(title)
            if p.get("missing") or "revisions" not in p:
                yield title, None, None
                continue
            rev = p["revisions"][0]
            yield title, rev["slots"]["main"].get("content", ""), rev.get("revid")
        # Titles the API normalised away (underscores, case) would otherwise
        # vanish without a trace and look like a successful fetch of nothing.
        for norm in d.get("query", {}).get("normalized", []):
            if norm.get("to") in seen:
                continue
            yield norm.get("from", "?"), None, None

    def redirect_targets(self, titles: list[str]) -> dict[str, str]:
        """alias -> target.

        `list=allredirects` cannot do this: it returns the *targets*, not the
        sources, so it yields no mapping at all. The working form is
        `titles=… & redirects=1`, which makes the API resolve them and report
        `from`/`to` pairs.
        """
        out: dict[str, str] = {}
        for i in range(0, len(titles), TITLES_LIMIT):
            d = self.get(
                action="query", titles="|".join(titles[i: i + TITLES_LIMIT]), redirects=1
            )
            for r in d.get("query", {}).get("redirects", []):
                if r.get("from") and r.get("to"):
                    out[r["from"]] = r["to"]
        return out

    def links(self, titles: list[str], ns: int = 0) -> dict[str, list[str]]:
        out: dict[str, list[str]] = {}
        for i in range(0, len(titles), TITLES_LIMIT):
            for q in self.paged(
                action="query", titles="|".join(titles[i: i + TITLES_LIMIT]),
                prop="links", plnamespace=ns, pllimit=LIST_LIMIT,
            ):
                for p in q.get("pages", []):
                    links = [l["title"] for l in p.get("links", [])]
                    if links:
                        out.setdefault(p["title"], []).extend(links)
        return {k: sorted(set(v)) for k, v in out.items()}

    # ---- change feed ----

    def recentchanges(
        self, namespaces: Iterable[int], *, types: str = "edit|new"
    ) -> Iterator[dict]:
        """Newest-first change feed. The caller stops at its watermark.

        Do not reach for `rcdir=newer` without a start point to get oldest-first:
        that walks the site's entire history from the beginning.
        """
        for q in self.paged(
            action="query", list="recentchanges",
            rcnamespace="|".join(str(n) for n in namespaces),
            rctype=types, rcprop="title|ids|timestamp", rclimit=LIST_LIMIT,
        ):
            yield from q.get("recentchanges", [])

    def latest_change_id(self, namespaces: Iterable[int]) -> int:
        d = self.get(
            action="query", list="recentchanges", rclimit=1, rcprop="ids",
            rcnamespace="|".join(str(n) for n in namespaces), rctype="edit|new",
        )
        rcs = d.get("query", {}).get("recentchanges", [])
        return int(rcs[0]["rcid"]) if rcs else 0
