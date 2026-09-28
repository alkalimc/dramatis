import { useId, useState } from "react";
import type { TaskId, TaskSummary } from "../api";
import { useT } from "../i18n";
import { authorName, when } from "../lib/format";
import { speechOnly } from "../lib/scene";
import { useQuery } from "../lib/useQuery";
import { useActions, useApi, useDispatch, useStore } from "../store";
import { CitationList } from "./Citation";

/** Every request, pinned questions (case files) first. */
export function CasesDrawer() {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const dispatch = useDispatch();
  const version = useStore((s) => s.worldVersion);
  const { data: tasks, reload } = useQuery(() => api.commands.tasks(), [api, version]);
  const id = useId();
  const [q, setQ] = useState("");

  const pin = async () => {
    if (!q.trim()) return;
    const task = await actions.attempt(api.commands.openCase(q.trim()));
    if (!task) return;
    setQ("");
    dispatch({ type: "drawer", drawer: { kind: "case", id: task } });
  };

  const roots = (tasks ?? []).filter((x) => x.parent === null);
  const pinned = roots.filter((x) => x.pinned);
  const other = roots.filter((x) => !x.pinned);

  return (
    <div className="drawer-body">
      <header className="drawer-head">
        <h2>{t("cases.title")}</h2>
      </header>
      <form
        className="row"
        onSubmit={(e) => {
          e.preventDefault();
          void pin();
        }}
      >
        <label htmlFor={id} className="sr-only">
          {t("cases.open")}
        </label>
        <input id={id} value={q} placeholder={t("cases.open.placeholder")} onChange={(e) => setQ(e.target.value)} />
        <button type="submit">{t("cases.open.submit")}</button>
      </form>
      {tasks && roots.length === 0 && <p className="muted">{t("cases.empty")}</p>}
      {pinned.length > 0 && (
        <section>
          <h3>{t("cases.pinned")}</h3>
          <TaskList tasks={pinned} />
        </section>
      )}
      {other.length > 0 && (
        <section>
          <h3>{t("cases.other")}</h3>
          <TaskList
            tasks={other}
            onPin={async (task) => {
              if ((await actions.attempt(api.commands.pinTask(task))) !== undefined) reload();
            }}
          />
        </section>
      )}
    </div>
  );
}

function TaskList({ tasks, onPin }: { tasks: TaskSummary[]; onPin?: (id: TaskId) => void }) {
  const t = useT();
  const dispatch = useDispatch();
  const actions = useActions();
  return (
    <ul className="tasks">
      {tasks.map((task) => (
        <li key={task.id}>
          {task.pinned ? (
            <button type="button" className="link name" onClick={() => dispatch({ type: "drawer", drawer: { kind: "case", id: task.id } })}>
              {task.question}
            </button>
          ) : task.channel ? (
            <button type="button" className="link name" onClick={() => actions.open(task.channel)}>
              {task.question}
            </button>
          ) : (
            <span className="name">{task.question}</span>
          )}
          <span className="muted small">
            {task.assignee ? `${task.assignee.name} · ` : task.pinned ? "" : `${t("cases.nobody")} · `}
            {task.pinned ? when(task.created_at) : t(task.status === "done" ? "cases.done" : "cases.active")}
          </span>
          {onPin && (
            <button type="button" className="link" onClick={() => onPin(task.id)}>
              {t("cases.pin")}
            </button>
          )}
        </li>
      ))}
    </ul>
  );
}

/** One pinned question: what was found, from whom, and the user's own conclusion. */
export function CaseDrawer({ id }: { id: TaskId }) {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const dispatch = useDispatch();
  const names = useStore((s) => s.names);
  const user = useStore((s) => s.settings?.user_name ?? null);
  const version = useStore((s) => s.worldVersion);
  const marker = useStore((s) => s.wording.scene_marker);
  const { data: cf, reload } = useQuery(() => api.commands.caseFile(id), [api, id, version]);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const cid = useId();

  if (!cf) return <p className="muted drawer-body">{t("app.loading")}</p>;

  const save = async () => {
    if (!draft.trim()) return;
    if ((await actions.attempt(api.commands.writeConclusion(id, draft.trim()))) !== undefined) {
      setEditing(false);
      reload();
    }
  };
  const unpin = async () => {
    if ((await actions.attempt(api.commands.unpinTask(id))) !== undefined) dispatch({ type: "drawer", drawer: { kind: "cases" } });
  };

  return (
    <div className="drawer-body case">
      <button type="button" className="link" onClick={() => dispatch({ type: "drawer", drawer: { kind: "cases" } })}>
        ← {t("case.back")}
      </button>
      <header className="drawer-head">
        <p className="muted small">{t("case.question")}</p>
        <h2>{cf.task.question}</h2>
        <span className="tag">{t("cases.pinned")}</span>
      </header>

      <section>
        <h3>{t("case.found")}</h3>
        {cf.trail.length === 0 ? (
          <p className="muted">{t("case.found.empty")}</p>
        ) : (
          <ul className="trail">
            {cf.trail.map((e) => (
              <li key={e.task.id}>
                <p className="muted small">{e.task.question}</p>
                {e.reply ? (
                  <>
                    <p>
                      {speechOnly(e.reply.text, marker)}{" "}
                      <span className="muted">{t("case.from", { name: e.task.assignee?.name ?? t("person.unknown") })}</span>
                    </p>
                    {e.reply.cites.length > 0 && (
                      <details>
                        <summary>{t("message.sources", { count: e.reply.cites.length })}</summary>
                        <CitationList cites={e.reply.cites} />
                      </details>
                    )}
                  </>
                ) : (
                  <p className="muted">{t("case.waiting", { name: e.task.assignee?.name ?? t("person.unknown") })}</p>
                )}
              </li>
            ))}
          </ul>
        )}
      </section>

      <section>
        <h3>
          <label htmlFor={cid}>{t("case.conclusion")}</label>
        </h3>
        {editing ? (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void save();
            }}
          >
            <textarea id={cid} rows={3} value={draft} onChange={(e) => setDraft(e.target.value)} autoFocus />
            <p className="row end">
              <button type="button" className="ghost" onClick={() => setEditing(false)}>
                {t("case.conclusion.cancel")}
              </button>
              <button type="submit" disabled={!draft.trim()}>
                {t("case.conclusion.save")}
              </button>
            </p>
          </form>
        ) : (
          <p>{cf.conclusion ? cf.conclusion.text : <span className="muted">{t("case.conclusion.empty")}</span>}</p>
        )}
        {cf.reactions.length > 0 && (
          <ul className="reactions" aria-label={t("case.reactions")}>
            {cf.reactions.map((r) => (
              <li key={r.message}>
                <button type="button" className="link" onClick={() => actions.open(r.channel)}>
                  {authorName(r.author, t, names, user)}
                </button>
                : {r.excerpt}
              </li>
            ))}
          </ul>
        )}
      </section>

      <p className="row wrap">
        <button type="button" onClick={() => dispatch({ type: "ask", draft: { question: cf.task.question, parent: id } })}>
          {t("case.ask_again")}
        </button>
        {!editing && (
          <button
            type="button"
            className="ghost"
            onClick={() => {
              setDraft(cf.conclusion?.text ?? "");
              setEditing(true);
            }}
          >
            {cf.conclusion ? t("case.edit_conclusion") : t("case.write_conclusion")}
          </button>
        )}
        <button type="button" className="ghost" onClick={unpin}>
          {t("case.unpin")}
        </button>
      </p>
    </div>
  );
}
