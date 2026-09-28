import { useId, useState } from "react";
import type { Candidate, RosterGroup } from "../api";
import { useT } from "../i18n";
import { activityText } from "../lib/format";
import { useActions, useApi, useDispatch, useStore } from "../store";
import { Avatar } from "../ui/Avatar";
import { ModeSwitch, modeHintKey } from "../ui/ModeSwitch";

export function RosterDrawer() {
  const t = useT();
  const roster = useStore((s) => s.roster);
  const dispatch = useDispatch();
  return (
    <div className="drawer-body">
      <header className="drawer-head">
        <h2>{t("roster.title")}</h2>
        {roster && <p className="muted">{t("roster.counts", { total: roster.total, enabled: roster.enabled })}</p>}
        <button type="button" className="ghost" onClick={() => dispatch({ type: "groupDialog", open: true })}>
          {t("roster.new_group")}
        </button>
      </header>
      <GroupList />
      <RosterList />
    </div>
  );
}

function GroupList() {
  const t = useT();
  const actions = useActions();
  const groups = useStore((s) => s.channels).filter((c) => c.kind === "group");
  return (
    <section className="groups" aria-labelledby="groups-h">
      <h3 id="groups-h">{t("group.list")}</h3>
      {groups.length === 0 ? (
        <p className="muted">{t("group.list.empty")}</p>
      ) : (
        <ul>
          {groups.map((g) => (
            <li key={g.id}>
              <button type="button" className="link name" onClick={() => actions.open(g.id)}>
                {g.topic ?? t("channel.group.untitled")}
              </button>{" "}
              <span className="muted small">
                {g.participants.map((p) => p.name).join(" · ")} · {t(`mode.${g.mode}`)}
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

/** The roster grouped by band. Rows carry no numbers; disabled people sit collapsed. */
export function RosterList() {
  const t = useT();
  const roster = useStore((s) => s.roster);
  return (
    <section className="roster" aria-label={t("roster.title")}>
      <FindByTopic />
      <details className="mode-legend">
        <summary>{t("roster.modes")}</summary>
        <dl>
          {(["enabled", "frozen", "disabled"] as const).map((m) => (
            <div key={m}>
              <dt>{t(`mode.${m}`)}</dt>
              <dd>{t(modeHintKey(m, "person"))}</dd>
            </div>
          ))}
        </dl>
      </details>
      {!roster || roster.groups.length === 0 ? (
        <p className="muted">{t("roster.empty")}</p>
      ) : (
        roster.groups.map((g) => <Band key={g.band} group={g} />)
      )}
    </section>
  );
}

function Band({ group }: { group: RosterGroup }) {
  const t = useT();
  const title = t(`roster.band.${group.band}`, { count: group.rows.length });
  const rows = (
    <ul className="roster-rows">
      {group.rows.map((r) => (
        <Row key={r.person.id} row={r} />
      ))}
    </ul>
  );
  // The closest people are what the user came for; everything else folds.
  if (group.band === "close") {
    return (
      <section className="band">
        <h3>{title}</h3>
        {rows}
      </section>
    );
  }
  return (
    <details className={`band band-${group.band}`}>
      <summary>
        <h3>{title}</h3>
      </summary>
      {rows}
    </details>
  );
}

function Row({ row }: { row: RosterGroup["rows"][number] }) {
  const t = useT();
  const actions = useActions();
  const dispatch = useDispatch();
  const off = row.mode === "disabled";
  return (
    <li className={`roster-row${off ? " off" : ""}`}>
      <Avatar person={row.person} />
      <div className="who">
        <button type="button" className="link name" onClick={() => dispatch({ type: "drawer", drawer: { kind: "person", id: row.person.id } })}>
          {row.person.name}
        </button>
        {row.recent && <span className="muted">{activityText(row.recent, t)}</span>}
        {row.companions.length > 0 && (
          <span className="muted small">{t("roster.companions", { names: row.companions.map((c) => c.name).join(" · ") })}</span>
        )}
      </div>
      {!off && (
        <button type="button" className="ghost" onClick={() => actions.openDirect(row.person.id)}>
          {t("person.talk")}
        </button>
      )}
      <ModeSwitch
        mode={row.mode}
        hints="none"
        label={t("mode.label", { name: row.person.name })}
        onChange={(m) => actions.setMode({ kind: "person", id: row.person.id }, m)}
      />
    </li>
  );
}

/** Search by what you need, not by name: a roster retrieval, no model call. */
function FindByTopic() {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const dispatch = useDispatch();
  const id = useId();
  const [topic, setTopic] = useState("");
  const [found, setFound] = useState<Candidate[] | null>(null);
  const run = async () => {
    if (!topic.trim()) return;
    setFound((await actions.attempt(api.commands.findPeople(topic.trim()))) ?? []);
  };
  return (
    <div className="find">
      <form
        role="search"
        className="row"
        onSubmit={(e) => {
          e.preventDefault();
          void run();
        }}
      >
        <label htmlFor={id} className="sr-only">
          {t("roster.find")}
        </label>
        <input id={id} type="search" value={topic} placeholder={t("roster.find.placeholder")} onChange={(e) => setTopic(e.target.value)} />
        <button type="submit">{t("roster.find.submit")}</button>
      </form>
      {found &&
        (found.length === 0 ? (
          <p className="muted">{t("select.who.empty")}</p>
        ) : (
          <ul className="candidates">
            {found.map((c) => (
              <li key={c.person.id}>
                <button type="button" className="link name" onClick={() => actions.openDirect(c.person.id)}>
                  {c.person.name}
                </button>
                <span className="reason">{c.reason}</span>
                <button type="button" className="link" onClick={() => dispatch({ type: "ask", draft: { question: topic, person: c.person.id } })}>
                  {t("person.ask")}
                </button>
              </li>
            ))}
          </ul>
        ))}
    </div>
  );
}
