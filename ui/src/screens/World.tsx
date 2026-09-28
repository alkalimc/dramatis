import { useId, useState } from "react";
import type { FactScope, FactView } from "../api";
import { useT } from "../i18n";
import { audienceText, when } from "../lib/format";
import { useQuery } from "../lib/useQuery";
import { useActions, useApi, useStore } from "../store";

const SCOPES: FactScope[] = ["all", "world", "own", "commitment", "hurt", "deleted"];

/** Every emergent fact with who can recall it: the one place visibility surfaces. */
export function WorldDrawer() {
  const t = useT();
  const api = useApi();
  const version = useStore((s) => s.worldVersion);
  const [scope, setScope] = useState<FactScope>("all");
  const { data: list, reload } = useQuery(() => api.commands.facts({ scope, person: null, task: null }), [api, scope, version]);

  return (
    <div className="drawer-body world">
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
      {list && list.items.length === 0 && <p className="muted">{t("world.empty")}</p>}
      <ul className="facts">
        {list?.items.map((f) => (
          <FactRow key={f.id} fact={f} onChange={reload} />
        ))}
      </ul>
    </div>
  );
}

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
            <span>{t("world.due", { when: when(fact.due) })}</span>
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
