import { useEffect, useRef, useState } from "react";
import type { Api, Citation, CitationView } from "../api";
import { useT } from "../i18n";
import { useApi } from "../store";
import { Icon } from "../ui/Icon";

// Resolving a citation is a local lookup, never a model call; one request per citation.
const cache = new Map<string, Promise<CitationView | null>>();

function resolve(api: Api, c: Citation): Promise<CitationView | null> {
  const key = `${c.chunk_id}@${c.revid}`;
  let p = cache.get(key);
  if (!p) {
    // A lookup that failed (or threw) is `null`, and is not remembered: the next try asks again.
    p = api.commands
      .resolveCitation(c)
      .then((r) => (r.status === "ok" ? r.data : null))
      .catch(() => null)
      .then((v) => {
        if (v === null) cache.delete(key);
        return v;
      });
    cache.set(key, p);
  }
  return p;
}

/** `null` while the sources are being looked up; a source that could not be found is `null` inside. */
export function useCitations(cites: Citation[]): (CitationView | null)[] | null {
  const api = useApi();
  const [views, setViews] = useState<(CitationView | null)[] | null>(cites.length ? null : []);
  const key = cites.map((c) => `${c.chunk_id}@${c.revid}`).join(",");
  useEffect(() => {
    let live = true;
    void Promise.all(cites.map((c) => resolve(api, c))).then((v) => live && setViews(v));
    return () => {
      live = false;
    };
    // `key` stands for `cites`, which is a fresh array on every render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, key]);
  return views;
}

/** Everything about one source: page, revision, passage, links out. */
export function CitationDetail({ citation, view }: { citation: Citation; view: CitationView | null }) {
  const t = useT();
  if (view === null) return <p className="muted">{t("citation.loading")}</p>;
  return (
    <div className="citation-detail">
      <dl className="citation-meta">
        <div>
          <dt>{t("citation.page")}</dt>
          <dd>{citation.page}</dd>
        </div>
        <div>
          <dt>{t("citation.revid")}</dt>
          <dd>{citation.revid}</dd>
        </div>
      </dl>
      {view.status === "updated" && <p className="muted small">{t("citation.stale")}</p>}
      {view.status === "gone" && <p className="muted small">{t("citation.gone")}</p>}
      {view.passage && (
        <blockquote className="passage">
          {view.passage.header && <p className="passage-header">{view.passage.header}</p>}
          <p>{view.passage.text}</p>
        </blockquote>
      )}
      <p className="citation-links">
        {view.source_url && (
          <a href={view.source_url} target="_blank" rel="noopener noreferrer">
            {t("citation.open")}
          </a>
        )}
        {view.revision_url && (
          <a href={view.revision_url} target="_blank" rel="noopener noreferrer">
            {t("citation.open_revision")}
          </a>
        )}
      </p>
    </div>
  );
}

/**
 * The second fold of a reply's sources: one line per entry (title and page); an entry opens
 * to its original text and page link, and closes again on a second click.
 */
export function SourceEntries({ cites }: { cites: Citation[] }) {
  const t = useT();
  const views = useCitations(cites);
  const [open, setOpen] = useState<number | null>(null);
  return (
    <ol className="sources-list">
      {cites.map((c, i) => {
        const v = views ? views[i] : null;
        const title = v?.passage?.header || c.page;
        const here = open === i;
        return (
          <li key={`${c.chunk_id}-${i}`} className="source">
            <button type="button" className="source-line" aria-expanded={here} aria-controls={`source-${c.chunk_id}-${i}`} onClick={() => setOpen(here ? null : i)}>
              <span className="source-title">{title}</span>
              {title !== c.page && <span className="muted small">{c.page}</span>}
            </button>
            {here && (
              <div id={`source-${c.chunk_id}-${i}`} className="source-body">
                {views === null ? (
                  <p className="muted small">{t("citation.loading")}</p>
                ) : v === null ? (
                  <p className="muted small">{t("citation.unavailable")}</p>
                ) : (
                  <>
                    {v.status === "updated" && <p className="muted small">{t("citation.stale")}</p>}
                    {v.status === "gone" && <p className="muted small">{t("citation.gone")}</p>}
                    {v.passage && <blockquote className="passage">{v.passage.text}</blockquote>}
                    <p className="citation-links">
                      {v.source_url && (
                        <a href={v.source_url} target="_blank" rel="noopener noreferrer">
                          {t("citation.open")}
                        </a>
                      )}
                      {v.revision_url && (
                        <a href={v.revision_url} target="_blank" rel="noopener noreferrer">
                          {t("citation.open_revision")}
                        </a>
                      )}
                    </p>
                  </>
                )}
              </div>
            )}
          </li>
        );
      })}
    </ol>
  );
}

/**
 * What a message cites, in a panel under it: for each source the page title, the original
 * wording of the cited passage and a link to the page.
 */
export function SourcesPanel({ cites, onClose }: { cites: Citation[]; onClose: () => void }) {
  const t = useT();
  const ref = useRef<HTMLElement>(null);

  useEffect(() => {
    ref.current?.focus({ preventScroll: true });
    // It opens under the message; bring it into view if that is past the end of the log.
    ref.current?.scrollIntoView?.({ block: "nearest" });
  }, []);

  return (
    <aside ref={ref} className="sources-panel" tabIndex={-1} aria-label={t("sources.title")} onKeyDown={(e) => e.key === "Escape" && onClose()}>
      <header className="sources-head">
        <h4>{t("sources.title")}</h4>
        <button type="button" className="round small" aria-label={t("drawer.close")} title={t("drawer.close")} onClick={onClose}>
          <Icon name="close" size={16} />
        </button>
      </header>
      <SourceEntries cites={cites} />
    </aside>
  );
}

export function CitationList({ cites }: { cites: Citation[] }) {
  return <SourceEntries cites={cites} />;
}
