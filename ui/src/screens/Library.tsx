import { useId, useState } from "react";
import type { SearchResults } from "../api";
import { useT } from "../i18n";
import { useActions, useApi, useDispatch, useStore } from "../store";
import { RosterList } from "./Roster";

/**
 * The library form: no chat endpoint, so the roster and full-text search are the main
 * screen. The host does not pretend to speak; it says, once, that it is not connected.
 */
export function Library() {
  const t = useT();
  const dispatch = useDispatch();
  const dismissed = useStore((s) => s.libraryNoticeDismissed);
  return (
    <div className="main library">
      {!dismissed && (
        <section className="card notice" aria-label={t("host.name")}>
          <p>
            <strong>{t("host.name")}</strong> {t("library.not_connected")}
          </p>
          <p className="row">
            <button type="button" onClick={() => dispatch({ type: "drawer", drawer: { kind: "system", tab: "endpoints" } })}>
              {t("library.configure")}
            </button>
            <button type="button" className="ghost" onClick={() => dispatch({ type: "dismissLibraryNotice" })}>
              {t("library.later")}
            </button>
          </p>
        </section>
      )}
      <CorpusSearch />
      <RosterList />
    </div>
  );
}

function CorpusSearch() {
  const t = useT();
  const api = useApi();
  const { attempt } = useActions();
  const id = useId();
  const [q, setQ] = useState("");
  const [results, setResults] = useState<SearchResults | null>(null);

  const run = async () => {
    if (!q.trim()) return;
    const r = await attempt(api.commands.searchCorpus(q.trim(), []));
    if (r) setResults(r);
  };

  return (
    <section className="search" aria-labelledby={`${id}-h`}>
      <h2 id={`${id}-h`}>{t("library.search")}</h2>
      <form
        role="search"
        className="row"
        onSubmit={(e) => {
          e.preventDefault();
          void run();
        }}
      >
        <label htmlFor={id} className="sr-only">
          {t("library.search")}
        </label>
        <input id={id} type="search" value={q} placeholder={t("library.search.placeholder")} onChange={(e) => setQ(e.target.value)} />
        <button type="submit">{t("library.search.submit")}</button>
      </form>
      {results && (
        <div className="results" aria-live="polite">
          {results.understood_as && <p className="muted small">{t("library.understood", { name: results.understood_as })}</p>}
          {results.confidence === "low" && <p className="tag thin">{t("confidence.low")}</p>}
          {results.hits.length === 0 ? (
            <p className="muted">{t("library.results.empty")}</p>
          ) : (
            <ol className="hits">
              {results.hits.map((h) => (
                <li key={h.citation.chunk_id}>
                  <article className="passage-card">
                    <h3>{h.header || h.citation.page}</h3>
                    <p>{h.text}</p>
                    <p className="muted small">
                      {h.persons.map((p) => p.name).join(" · ")}
                      {h.source_url && (
                        <>
                          {" · "}
                          <a href={h.source_url} target="_blank" rel="noopener noreferrer">
                            {t("citation.open")}
                          </a>
                        </>
                      )}
                    </p>
                  </article>
                </li>
              ))}
            </ol>
          )}
        </div>
      )}
    </section>
  );
}
