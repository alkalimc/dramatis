import { useEffect, useState } from "react";
import type { Api, Citation, CitationView } from "../api";
import { useT } from "../i18n";
import { useApi } from "../store";

// Resolving a citation is a local lookup, never a model call; one request per citation.
const cache = new Map<string, Promise<CitationView | null>>();

function resolve(api: Api, c: Citation): Promise<CitationView | null> {
  const key = `${c.chunk_id}@${c.revid}`;
  let p = cache.get(key);
  if (!p) {
    p = api.commands.resolveCitation(c).then((r) => (r.status === "ok" ? r.data : null));
    cache.set(key, p);
  }
  return p;
}

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

export function CitationList({ cites }: { cites: Citation[] }) {
  const views = useCitations(cites);
  return (
    <ol className="citation-list">
      {cites.map((c, i) => (
        <li key={`${c.chunk_id}-${i}`}>
          <CitationDetail citation={c} view={views ? views[i] : null} />
        </li>
      ))}
    </ol>
  );
}
