import { useId, useRef, useState } from "react";
import type { DailyEvent, FactScope, FactView } from "../api";
import { useT } from "../i18n";
import { audienceText, when } from "../lib/format";
import { useQuery } from "../lib/useQuery";
import { recentByCategory } from "../lib/world";
import { BackToTop } from "../ui/BackToTop";
import { InlineError } from "../ui/ErrorBar";
import { useActions, useApi, useStore } from "../store";

const SCOPES: FactScope[] = ["all", "world", "own", "commitment", "hurt", "deleted"];

/** Every emergent fact with who can recall it: the one place visibility surfaces. */
export function WorldPage() {
  const t = useT();
  const api = useApi();
  const version = useStore((s) => s.worldVersion);
  const [scope, setScope] = useState<FactScope>("all");
  const scroll = useRef<HTMLDivElement>(null);
  const { data: list, error, loading, reload } = useQuery(() => api.commands.facts({ scope, person: null }), [api, scope, version]);
  const events = useQuery(() => api.commands.dailyEvents(scope === "deleted"), [api, scope, version]);
  // A happening is one memory with two views, its card and the fact that tells people of it;
  // where the cards show, the fact is left out of the list.
  const refresh = () => {
    reload();
    events.reload();
  };
  const withEvents = scope === "all" || scope === "world" || scope === "deleted";
  const told = new Set(withEvents ? (events.data ?? []).flatMap((e) => (e.fact ? [e.fact] : [])) : []);
  const items = (list?.items ?? []).filter((f) => !told.has(f.id));
  // The newest of each category sit in cards on top; the list under them is the rest.
  const groups = scope === "all" ? recentByCategory(items) : [];
  const carded = new Set(groups.flatMap((g) => g.items.map((f) => f.id)));
  const rest = items.filter((f) => !carded.has(f.id));

  return (
    <div className="main page">
      <div ref={scroll} className="page-scroll">
        <div className="drawer-body page-body world">
          <header className="drawer-head">
            <h2>{t("world.title")}</h2>
            {list && <p className="muted">{t("world.counts", { total: list.total, deleted: list.deleted })}</p>}
          </header>
          <div className="chips" role="group" aria-label={t("world.filter")}>
            {SCOPES.map((s) => (
              <button key={s} type="button" className={`chip${scope === s ? " on" : ""}`} aria-pressed={scope === s} onClick={() => setScope(s)}>
                {t(`world.scope.${s}`)}
              </button>
            ))}
          </div>
          {error && <InlineError error={error} onRetry={reload} />}
          {loading && !list && (
            <p className="muted" role="status">
              {t("app.loading")}
            </p>
          )}
          {/* The daily events are world memories: they show with the world's and the deleted ones. */}
          <Lately events={withEvents ? (events.data ?? []) : []} groups={groups} onChange={refresh} />
          {list && items.length === 0 && (events.data ?? []).length === 0 && <p className="muted">{t("world.empty")}</p>}
          {groups.length > 0 && rest.length > 0 && <h3>{t("world.earlier")}</h3>}
          <ul className="facts">
            {rest.map((f) => (
              <FactRow key={f.id} fact={f} onChange={refresh} />
            ))}
          </ul>
        </div>
      </div>
      <BackToTop scroller={scroll} />
    </div>
  );
}

/**
 * "On board lately": the happenings the host told of, newest first, and under them the
 * newest memories of each category as cards, every one deletable. Absent while empty.
 */
function Lately({ events, groups, onChange }: { events: DailyEvent[]; groups: ReturnType<typeof recentByCategory>; onChange: () => void }) {
  const t = useT();
  if (events.length === 0 && groups.length === 0) return null;
  return (
    <section className="event-log" aria-label={t("world.events.title")}>
      <h3>{t("world.events.title")}</h3>
      {events.length > 0 && (
        <div className="recent-group" role="group" aria-label={t("world.recent.events")}>
          {groups.length > 0 && <h4>{t("world.recent.events")}</h4>}
          <ul className="facts">
            {events.map((e) => (
              <EventCard key={e.id} event={e} onChange={onChange} />
            ))}
          </ul>
        </div>
      )}
      {groups.map((g) => (
        <div key={g.category} className="recent-group" role="group" aria-label={t(CATEGORY_LABEL[g.category])}>
          <h4>{t(CATEGORY_LABEL[g.category])}</h4>
          <ul className="facts">
            {g.items.map((f) => (
              <FactRow key={f.id} fact={f} onChange={onChange} />
            ))}
          </ul>
        </div>
      ))}
    </section>
  );
}

/** One happening. Deleting hides it (and the fact that tells of it); a deleted one can be restored. */
function EventCard({ event: e, onChange }: { event: DailyEvent; onChange: () => void }) {
  const t = useT();
  const api = useApi();
  const { attempt } = useActions();
  const run = async (p: ReturnType<typeof api.commands.deleteEvent>) => {
    if ((await attempt(p)) !== undefined) onChange();
  };
  return (
    <li className={`fact${e.deleted ? " retracted" : ""}`}>
      <p className="fact-text">
        {e.deleted ? <del>{e.title}</del> : <strong>{e.title}</strong>} {!e.deleted && e.text}
        {e.deleted && <span className="sr-only"> ({t("world.deleted")})</span>}
      </p>
      <p className="fact-meta">
        <span>{e.day}</span>
        {" · "}
        <span>{t("world.events.people", { names: e.persons.map((p) => p.name).join(", ") })}</span>
      </p>
      <p className="row fact-actions">
        {e.deleted ? (
          <button type="button" className="ghost" onClick={() => run(api.commands.restoreEvent(e.id))}>
            {t("world.restore")}
          </button>
        ) : (
          <button type="button" className="ghost" onClick={() => run(api.commands.deleteEvent(e.id))}>
            {t("world.delete")}
          </button>
        )}
      </p>
    </li>
  );
}

const CATEGORY_LABEL = {
  world: "world.scope.world",
  said: "world.kind.fact",
  own: "world.scope.own",
  commitment: "world.scope.commitment",
  hurt: "world.scope.hurt",
} as const;

function FactRow({ fact, onChange }: { fact: FactView; onChange: () => void }) {
  const t = useT();
  const api = useApi();
  const { attempt } = useActions();
  const names = useStore((s) => s.names);
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(fact.text);
  const id = useId();

  const run = async (p: ReturnType<typeof api.commands.deleteFact>) => {
    if ((await attempt(p)) !== undefined) onChange();
  };
  const save = async () => {
    if (!text.trim()) return;
    if ((await attempt(api.commands.editFact(fact.id, text.trim()))) !== undefined) {
      setEditing(false);
      onChange();
    }
  };

  const hurt = fact.kind === "hurt";
  const about = fact.about?.kind === "person" ? (names[fact.about.id] ?? t("person.unknown")) : t("person.unknown");
  // Only the user's own kinds of memory are re-wordable; a hurt is a quote, kept or dropped.
  const editable = !hurt && !fact.retracted;

  return (
    <li className={`fact${fact.retracted ? " retracted" : ""}`}>
      {editing ? (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void save();
          }}
        >
          <label htmlFor={id} className="sr-only">
            {t("world.edit.label")}
          </label>
          <textarea id={id} rows={2} value={text} onChange={(e) => setText(e.target.value)} autoFocus />
          <p className="row end">
            <button type="button" className="ghost" onClick={() => setEditing(false)}>
              {t("world.edit.cancel")}
            </button>
            <button type="submit">{t("world.edit.save")}</button>
          </p>
        </form>
      ) : (
        <p className="fact-text">
          {fact.retracted ? <del>{fact.text}</del> : hurt ? <q>{fact.text}</q> : fact.text}
          {fact.retracted && <span className="sr-only"> ({t("world.deleted")})</span>}
        </p>
      )}
      <p className="fact-meta">
        <span>{t(`world.kind.${fact.kind}`)}</span>
        {" · "}
        <span>{hurt ? t("world.hurt.effect", { name: about }) : audienceText(fact.audience, t)}</span>
        {fact.due && (
          <>
            {" · "}
            <span>{t("world.due", { when: when(fact.due, t) })}</span>
          </>
        )}
      </p>
      {!editing && (
        <p className="row fact-actions">
          {fact.retracted ? (
            <button type="button" className="ghost" onClick={() => run(api.commands.restoreFact(fact.id))}>
              {t("world.restore")}
            </button>
          ) : (
            <>
              {editable && (
                <button type="button" className="ghost" onClick={() => setEditing(true)}>
                  {t("world.edit")}
                </button>
              )}
              <button type="button" className="ghost" onClick={() => run(api.commands.deleteFact(fact.id))}>
                {t("world.delete")}
              </button>
            </>
          )}
        </p>
      )}
    </li>
  );
}
