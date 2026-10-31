import { useEffect, useId, useRef, useState } from "react";
import { settle, type Failure, type Passage, type SearchResults } from "../api";
import { useT } from "../i18n";
import { snippet, termsOf } from "../lib/snippet";
import { useQuery } from "../lib/useQuery";
import { useApi, useDispatch, useStore } from "../store";
import { BackToTop } from "../ui/BackToTop";
import { InlineError } from "../ui/ErrorBar";
import { ExternalLink } from "../ui/ExternalLink";
import { Highlight } from "../ui/Highlight";

/** Hits shown at first, and added by each "show more". */
const PAGE = 30;

/**
 * The library: full-text search of the corpus, with a unit opened in place.
 * It is the main screen when no chat endpoint is set up, and one the user can open at any
 * time. Nothing here calls a model; without one the host says, once, that it is not connected.
 */
export function Library() {
  const t = useT();
  const dispatch = useDispatch();
  const dismissed = useStore((s) => s.libraryNoticeDismissed);
  const model = useStore((s) => s.status?.form === "full" && Boolean(s.hostChannel));
  const scroll = useRef<HTMLDivElement>(null);
  return (
    <div className="main library">
      <div ref={scroll} className="library-scroll">
      {!model && !dismissed && (
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
      </div>
      <BackToTop scroller={scroll} />
    </div>
  );
}

function CorpusSearch() {
  const t = useT();
  const api = useApi();
  const id = useId();
  const roster = useStore((s) => s.roster);
  const [q, setQ] = useState("");
  const [person, setPerson] = useState("");
  const [template, setTemplate] = useState("");
  const [templates, setTemplates] = useState<string[]>([]);
  const [results, setResults] = useState<SearchResults | null>(null);
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState<Failure | null>(null);
  const [reading, setReading] = useState<string | null>(null);
  // The words the shown results were found for, and how many of them are on screen.
  const [terms, setTerms] = useState<string[]>([]);
  const [shown, setShown] = useState(PAGE);
  const people = roster ? roster.groups.flatMap((g) => g.rows.map((r) => r.person)) : [];

  useEffect(() => {
    void settle(api.commands.corpusTemplates()).then((r) => r.ok && setTemplates(r.data));
  }, [api]);

  const run = async () => {
    if (!q.trim() || busy) return;
    setBusy(true);
    const r = await settle(api.commands.searchCorpus(q.trim(), person ? [person] : [], template ? [template] : []));
    setBusy(false);
    setFailed(r.ok ? null : r.error);
    if (r.ok) {
      setResults(r.data);
      setShown(PAGE);
      setTerms(termsOf([q, r.data.understood_as ?? ""].join(" ")));
    }
  };

  return (
    <section className="search" aria-labelledby={`${id}-h`}>
      <h2 id={`${id}-h`}>{t("library.search")}</h2>
      <form
        role="search"
        className="row field-row"
        onSubmit={(e) => {
          e.preventDefault();
          setReading(null);
          void run();
        }}
      >
        <label htmlFor={id} className="sr-only">
          {t("library.search")}
        </label>
        <input id={id} type="search" value={q} placeholder={t("library.search.placeholder")} onChange={(e) => setQ(e.target.value)} />
        <label className="sr-only" htmlFor={`${id}-person`}>
          {t("library.filter.person")}
        </label>
        <select id={`${id}-person`} value={person} onChange={(e) => setPerson(e.target.value)}>
          <option value="">{t("library.filter.person.any")}</option>
          {people.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </select>
        {templates.length > 0 && (
          <>
            <label className="sr-only" htmlFor={`${id}-template`}>
              {t("library.filter.template")}
            </label>
            <select id={`${id}-template`} value={template} onChange={(e) => setTemplate(e.target.value)}>
              <option value="">{t("library.filter.template.any")}</option>
              {templates.map((n) => (
                <option key={n} value={n}>
                  {n}
                </option>
              ))}
            </select>
          </>
        )}
        <button type="submit" disabled={busy} aria-busy={busy}>
          {t("library.search.submit")}
        </button>
      </form>
      {busy && (
        <p className="muted" role="status">
          {t("library.searching")}
        </p>
      )}
      {failed && <InlineError error={failed} onRetry={() => void run()} />}
      {reading ? (
        <UnitReader id={reading} onOpen={setReading} onBack={() => setReading(null)} back={results !== null} />
      ) : (
        results && (
          <div className="results" aria-live="polite">
            {results.understood_as && <p className="muted small">{t("library.understood", { name: results.understood_as })}</p>}
            {results.confidence === "low" && <p className="tag thin">{t("confidence.low")}</p>}
            {results.hits.length === 0 ? (
              <p className="muted">{t("library.results.empty")}</p>
            ) : (
              <>
                <ol className="hits">
                  {results.hits.slice(0, shown).map((h) => (
                    <li key={h.citation.chunk_id}>
                      <article className="passage-card hit">
                        <header className="hit-head">
                          <h3>
                            <Highlight text={h.header || h.citation.page} terms={terms} />
                          </h3>
                          <span className="tag">{h.template}</span>
                        </header>
                        {h.persons.length > 0 && <p className="hit-people muted small">{h.persons.map((p) => p.name).join(" · ")}</p>}
                        <p className="hit-snippet">
                          <Highlight text={snippet(h.text, terms, 120)} terms={terms} />
                        </p>
                        <footer className="hit-foot">
                          <button type="button" className="ghost" onClick={() => setReading(h.citation.chunk_id)}>
                            {t("library.read")}
                          </button>
                          {h.source_url && <ExternalLink href={h.source_url}>{t("citation.open")}</ExternalLink>}
                        </footer>
                      </article>
                    </li>
                  ))}
                </ol>
                {results.hits.length > shown && (
                  <p className="row end">
                    <button type="button" className="ghost" onClick={() => setShown(shown + PAGE)}>
                      {t("library.more", { count: Math.min(PAGE, results.hits.length - shown) })}
                    </button>
                  </p>
                )}
              </>
            )}
          </div>
        )
      )}
    </section>
  );
}

/** A unit in full with the units next to it in the same source; a neighbour opens in turn. */
function UnitReader({ id, onOpen, onBack, back }: { id: string; onOpen: (id: string) => void; onBack: () => void; back: boolean }) {
  const t = useT();
  const api = useApi();
  const { data, error, loading, reload } = useQuery(() => api.commands.readUnit(id, 2), [api, id]);
  const near = (p: Passage) => (
    <li key={p.citation.chunk_id} className="neighbour">
      <button type="button" className="link" onClick={() => onOpen(p.citation.chunk_id)}>
        {p.header || p.citation.page}
      </button>
      <p className="muted">{p.text}</p>
    </li>
  );
  return (
    <article className="reader" aria-label={t("library.reader")}>
      {back && (
        <button type="button" className="link" onClick={onBack}>
          {t("library.back")}
        </button>
      )}
      {loading && !data && (
        <p className="muted" role="status">
          {t("citation.loading")}
        </p>
      )}
      {error && <InlineError error={error} onRetry={reload} />}
      {data && (
        <>
          {data.before.length > 0 && <ul className="neighbours">{data.before.map(near)}</ul>}
          <section className="passage-card current">
            <h3>{data.unit.header || data.unit.citation.page}</h3>
            <p className="unit-text">{data.unit.text}</p>
            <p className="muted small">
              {data.unit.citation.page}
              {data.unit.persons.length > 0 && ` · ${data.unit.persons.map((p) => p.name).join(" · ")}`}
              {data.unit.source_url && (
                <>
                  {" · "}
                  <ExternalLink href={data.unit.source_url}>
                    {t("citation.open")}
                  </ExternalLink>
                </>
              )}
            </p>
          </section>
          {data.after.length > 0 && <ul className="neighbours">{data.after.map(near)}</ul>}
        </>
      )}
    </article>
  );
}
