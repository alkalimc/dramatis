import { useId, useState } from "react";
import type { Candidate, RosterGroup } from "../api";
import { useT, useUserName } from "../i18n";
import { activityText } from "../lib/format";
import { groupTitle } from "../lib/chats";
import { canTalk } from "../lib/library";
import { hasPersona } from "../lib/persona";
import { filtering, passes, type RosterFilter } from "../lib/roster";
import { useActions, useApi, useDispatch, useStore } from "../store";
import { Avatar, ChannelAvatar } from "../ui/Avatar";
import { ModeSwitch, modeHintKey } from "../ui/ModeSwitch";

/** A small "typing" marker for a channel that is generating. */
export function TypingMark() {
  const t = useT();
  return (
    <span className="typing-mark muted small" role="status">
      {t("typing.marker")}
    </span>
  );
}

export function RosterDrawer() {
  const t = useT();
  const roster = useStore((s) => s.roster);
  return (
    <div className="drawer-body">
      <header className="drawer-head">
        <h2>{t("roster.title")}</h2>
        {roster && <p className="muted">{t("roster.counts", { total: roster.total, enabled: roster.enabled })}</p>}
      </header>
      <GroupList />
      <RosterList />
    </div>
  );
}

function GroupList() {
  const t = useT();
  const actions = useActions();
  const me = useUserName();
  const groups = useStore((s) => s.channels).filter((c) => c.kind === "group");
  const typing = useStore((s) => s.typing);
  const typingIn = new Set(Object.keys(typing));
  return (
    <section className="groups" aria-labelledby="groups-h">
      <h3 id="groups-h">{t("group.list")}</h3>
      {groups.length === 0 ? (
        <p className="muted">{t("group.list.empty")}</p>
      ) : (
        <ul className="roster-rows">
          {groups.map((g) => (
            <li key={g.id} className="roster-row">
              <ChannelAvatar channel={g} />
              <div className="who">
                <button type="button" className="link name" onClick={() => actions.open(g.id)}>
                  {groupTitle(g, t, me)}
                </button>
                {typingIn.has(g.id) && <TypingMark />}
                <span className="muted small">
                  {g.participants.map((p) => p.name).join(" · ")} · {t(`mode.${g.mode}`)}
                </span>
              </div>
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
  const filter = useStore((s) => s.rosterFilter);
  const on = filtering(filter);
  const groups = (roster?.groups ?? [])
    .map((g) => ({ ...g, rows: g.rows.filter((r) => passes(r, filter)) }))
    .filter((g) => g.rows.length > 0);
  return (
    <section className="roster" aria-label={t("roster.title")}>
      <FindByTopic />
      <FilterBar />
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
      ) : groups.length === 0 ? (
        <p className="muted" role="status">
          {t("roster.filter.none")}
        </p>
      ) : (
        // A filter opens the folded bands: what it found is what the user asked for.
        groups.map((g) => <Band key={`${g.band}:${on}`} group={g} open={on} />)
      )}
    </section>
  );
}

function Band({ group, open }: { group: RosterGroup; open: boolean }) {
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
    <details className={`band band-${group.band}`} open={open || undefined}>
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
  const library = !useStore(canTalk);
  const off = row.mode === "disabled";
  const ready = hasPersona(row);
  const note = useId();
  const typing = useStore((s) => {
    const direct = s.channels.find((c) => c.kind === "direct" && c.participants.some((p) => p.id === row.person.id));
    return Boolean(direct && s.typing[direct.id]);
  });
  return (
    <li className={`roster-row${off ? " off" : ""}${ready ? "" : " unavailable"}`}>
      <Avatar person={row.person} />
      <div className="who">
        <button type="button" className="link name" onClick={() => dispatch({ type: "drawer", drawer: { kind: "person", id: row.person.id } })}>
          {row.person.name}
        </button>
        {typing && <TypingMark />}
        {!ready && (
          <span className="muted small" id={note}>
            {t("person.unavailable")}
          </span>
        )}
        {ready && row.recent && <span className="muted">{activityText(row.recent, t)}</span>}
        {row.companions.length > 0 && (
          <span className="muted small">{t("roster.companions", { names: row.companions.map((c) => c.name).join(" · ") })}</span>
        )}
      </div>
      {!off && !library && (
        <button type="button" className="ghost" disabled={!ready} aria-describedby={ready ? undefined : note} onClick={() => actions.openDirect(row.person.id)}>
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

/** Quick filters over the list: mode, contact, persona. One chip per choice, one choice per row. */
function FilterBar() {
  const t = useT();
  const dispatch = useDispatch();
  const filter = useStore((s) => s.rosterFilter);
  const set = (next: Partial<RosterFilter>) => dispatch({ type: "rosterFilter", filter: { ...filter, ...next } });
  const modes = ["all", "enabled", "frozen", "disabled"] as const;
  const talks = [
    ["any", t("roster.filter.all")],
    ["talked", t("roster.filter.talked")],
    ["never", t("roster.filter.never")],
  ] as const;
  return (
    <div className="roster-filters">
      <div className="chips" role="group" aria-label={t("roster.filter.mode")}>
        {modes.map((m) => (
          <button key={m} type="button" className={`chip${filter.mode === m ? " on" : ""}`} aria-pressed={filter.mode === m} onClick={() => set({ mode: m })}>
            {m === "all" ? t("roster.filter.all") : t(`mode.${m}`)}
          </button>
        ))}
      </div>
      <div className="chips" role="group" aria-label={t("roster.filter.talk")}>
        {talks.map(([k, label]) => (
          <button key={k} type="button" className={`chip${filter.talk === k ? " on" : ""}`} aria-pressed={filter.talk === k} onClick={() => set({ talk: k })}>
            {label}
          </button>
        ))}
        <button type="button" className={`chip${filter.persona ? " on" : ""}`} aria-pressed={filter.persona} onClick={() => set({ persona: !filter.persona })}>
          {t("roster.filter.persona")}
        </button>
      </div>
    </div>
  );
}

/** Search by what you need, not by name: a roster retrieval, no model call. */
function FindByTopic() {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const dispatch = useDispatch();
  const id = useId();
  const library = !useStore(canTalk);
  const filter = useStore((s) => s.rosterFilter);
  const roster = useStore((s) => s.roster);
  const [topic, setTopic] = useState("");
  const [found, setFound] = useState<Candidate[] | null>(null);
  const run = async () => {
    if (!topic.trim()) return;
    setFound((await actions.attempt(api.commands.findPeople(topic.trim()))) ?? []);
  };
  // The same filters narrow what the search found (a candidate not on the roster stays).
  const rows = new Map(roster?.groups.flatMap((g) => g.rows).map((r) => [r.person.id, r]));
  const shown = found?.filter((c) => {
    const row = rows.get(c.person.id);
    return row ? passes(row, filter) : passes({ mode: c.mode, talked: false, has_persona: true }, { ...filter, talk: "any" });
  });
  return (
    <div className="find">
      <form
        role="search"
        className="row field-row"
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
      {shown &&
        (shown.length === 0 ? (
          <p className="muted">{t("roster.find.empty")}</p>
        ) : (
          <ul className="candidates">
            {shown.map((c) => (
              <li key={c.person.id}>
                <button
                  type="button"
                  className="link name"
                  onClick={() => (library ? dispatch({ type: "drawer", drawer: { kind: "person", id: c.person.id } }) : actions.openDirect(c.person.id))}
                >
                  {c.person.name}
                </button>
                <span className="reason">{c.reason}</span>
              </li>
            ))}
          </ul>
        ))}
    </div>
  );
}
