import type { Message, Mode, PersonRef, Roster, Target } from "../api";
import { makeT } from "../i18n";
import { typingKey, type Action, type State } from "./state";

function withNames(names: State["names"], persons: PersonRef[]): State["names"] {
  let out = names;
  for (const p of persons) {
    if (out[p.id] !== p.name) {
      if (out === names) out = { ...names };
      out[p.id] = p.name;
    }
  }
  return out;
}

function rosterPersons(r: Roster): PersonRef[] {
  return r.groups.flatMap((g) => g.rows.flatMap((row) => [row.person, ...row.companions]));
}

function sameTarget(a: Target, b: Target): boolean {
  return a.kind === b.kind && a.id === b.id;
}

function setRosterMode(r: Roster | null, target: Target, mode: Mode): Roster | null {
  if (!r || target.kind !== "person") return r;
  let enabled = 0;
  const groups = r.groups.map((g) => ({
    ...g,
    rows: g.rows.map((row) => {
      const next = row.person.id === target.id ? { ...row, mode } : row;
      if (next.mode === "enabled") enabled += 1;
      return next;
    }),
  }));
  return { ...r, groups, enabled };
}

/** Insert or replace by id, keeping the channel's order by time then arrival. */
function upsert(list: Message[] | undefined, m: Message): Message[] {
  const cur = list ?? [];
  const i = cur.findIndex((x) => x.id === m.id);
  if (i >= 0) return cur.map((x, j) => (j === i ? m : x));
  const out = [...cur, m];
  // Events arrive in order almost always; only walk back when one did not.
  for (let j = out.length - 1; j > 0 && out[j - 1].at > out[j].at; j--) {
    [out[j - 1], out[j]] = [out[j], out[j - 1]];
  }
  return out;
}

function omit<R extends Partial<Record<string, unknown>>>(rec: R, key: string): R {
  if (!(key in rec)) return rec;
  const { [key]: _drop, ...rest } = rec;
  return rest as R;
}

/** Something came back in this channel: the user is no longer waiting on it. */
function answered(s: State, channel: string): State["awaiting"] {
  return omit(s.awaiting, channel);
}

export function reduce(s: State, a: Action): State {
  switch (a.type) {
    case "status":
      return { ...s, status: a.status };
    case "wording":
      return { ...s, wording: a.wording, t: makeT(a.wording) };
    case "clock":
      return { ...s, clock: a.clock };
    case "settings":
      return { ...s, settings: a.settings };
    case "endpoints":
      return { ...s, endpoints: a.endpoints };
    case "digest":
      return { ...s, digest: a.digest, names: withNames(s.names, a.digest.items.map((w) => w.person)) };
    case "coldstart":
      return { ...s, coldstart: a.candidates, names: withNames(s.names, a.candidates.map((c) => c.person)) };
    case "roster":
      return { ...s, roster: a.roster, names: withNames(s.names, rosterPersons(a.roster)) };
    case "quota":
      return { ...s, quota: a.quota };
    case "hostChannel":
      return { ...s, hostChannel: a.id };
    case "channels":
      return {
        ...s,
        channels: a.channels,
        names: withNames(s.names, a.channels.flatMap((c) => c.participants)),
      };
    case "names":
      return { ...s, names: withNames(s.names, a.persons) };
    case "history": {
      const cur = s.messages[a.channel] ?? [];
      const known = new Set(cur.map((m) => m.id));
      const fresh = a.messages.filter((m) => !known.has(m.id));
      let merged = a.older ? [...fresh, ...cur] : [...cur, ...fresh];
      // A reload after a write elsewhere can bring messages older than ones already shown.
      if (a.merge) merged = merged.map((m) => a.messages.find((x) => x.id === m.id) ?? m).sort((x, y) => Date.parse(x.at) - Date.parse(y.at));
      return {
        ...s,
        messages: { ...s.messages, [a.channel]: merged },
        historyDone: { ...s.historyDone, [a.channel]: a.done },
      };
    }
    case "messageDelta": {
      const e = a.event;
      const prev = s.streams[e.message];
      return {
        ...s,
        streams: {
          ...s.streams,
          [e.message]: { channel: e.channel, author: e.author, text: (prev?.text ?? "") + e.delta },
        },
        awaiting: answered(s, e.channel),
      };
    }
    case "messageAdded": {
      const pending = s.pendingActions[a.event.message.id];
      let m = a.event.message;
      if (pending && m.actions.length === 0) m = { ...m, actions: pending };
      return {
        ...s,
        messages: { ...s.messages, [m.channel]: upsert(s.messages[m.channel], m) },
        streams: omit(s.streams, m.id),
        pendingActions: omit(s.pendingActions, m.id),
        // The user's own turn echoed back is not an answer.
        awaiting: m.author.kind === "user" ? s.awaiting : answered(s, m.channel),
        unread: m.author.kind === "user" || m.channel === (s.channel ?? s.hostChannel) ? s.unread : { ...s.unread, [m.channel]: (s.unread[m.channel] ?? 0) + 1 },
      };
    }
    case "toolCalled": {
      const e = a.event;
      const list = s.messages[e.channel];
      const i = list?.findIndex((m) => m.id === e.message) ?? -1;
      if (list && i >= 0) {
        const m = list[i];
        const next = { ...m, actions: [...m.actions, e.action] };
        return { ...s, messages: { ...s.messages, [e.channel]: list.map((x, j) => (j === i ? next : x)) } };
      }
      // A reply that acts before it writes: show its action lines at once, under a
      // stream with no text yet.
      const stream = s.streams[e.message] ?? { channel: e.channel, author: e.author, text: "" };
      return {
        ...s,
        pendingActions: { ...s.pendingActions, [e.message]: [...(s.pendingActions[e.message] ?? []), e.action] },
        streams: s.streams[e.message] ? s.streams : { ...s.streams, [e.message]: stream },
        awaiting: answered(s, e.channel),
      };
    }
    case "turnActivity": {
      const { channel, author, state } = a.event;
      const key = typingKey(author);
      const here = s.typing[channel] ?? {};
      if (state.kind === "done" || state.kind === "failed") {
        // A failed turn never sends its message: drop the half-written bubble with it.
        const streams =
          state.kind === "failed"
            ? Object.fromEntries(
                Object.entries(s.streams).filter(([, v]) => v.channel !== channel || typingKey(v.author) !== key),
              )
            : s.streams;
        const failures =
          state.kind === "failed" ? { ...s.failures, [channel]: { author, reason: state.reason } } : omit(s.failures, channel);
        if (state.kind === "done" && !(key in here) && !s.failures[channel] && !s.awaiting[channel]) return s;
        const rest = omit(here, key);
        const typing = Object.keys(rest).length ? { ...s.typing, [channel]: rest } : omit(s.typing, channel);
        // Nothing more is coming for a turn that ended: the user's wait is over too.
        return { ...s, typing: key in here ? typing : s.typing, streams, failures, awaiting: answered(s, channel) };
      }
      const tool = state.kind === "tool";
      if (here[key]?.tool === tool && !s.failures[channel]) return s;
      return {
        ...s,
        typing: { ...s.typing, [channel]: { ...here, [key]: { author, tool } } },
        failures: omit(s.failures, channel),
        awaiting: answered(s, channel),
      };
    }
    case "modeChanged": {
      const { target, mode } = a.event;
      return {
        ...s,
        roster: setRosterMode(s.roster, target, mode),
        channels: s.channels.map((c) => (sameTarget(target, { kind: "channel", id: c.id }) ? { ...c, mode } : c)),
        coldstart: s.coldstart.map((c) =>
          sameTarget(target, { kind: "person", id: c.person.id }) ? { ...c, mode } : c,
        ),
        peopleVersion: s.peopleVersion + 1,
      };
    }
    case "worldChanged":
      return { ...s, worldVersion: s.worldVersion + 1 };
    case "rosterChanged":
      return { ...s, peopleVersion: s.peopleVersion + 1 };
    case "rewound": {
      const { channel, from } = a.event;
      const list = s.messages[channel];
      const i = list?.findIndex((m) => m.id === from) ?? -1;
      if (!list || i < 0) return s;
      const gone = new Set(list.slice(i).map((m) => m.id));
      const streams = Object.fromEntries(Object.entries(s.streams).filter(([id, v]) => v.channel !== channel && !gone.has(id)));
      const keep = <V,>(rec: Record<string, V>) => Object.fromEntries(Object.entries(rec).filter(([id]) => !gone.has(id)));
      return {
        ...s,
        messages: { ...s.messages, [channel]: list.slice(0, i) },
        streams,
        pendingActions: keep(s.pendingActions),
        sourcesOpen: keep(s.sourcesOpen),
        awaiting: answered(s, channel),
        typing: omit(s.typing, channel),
        failures: omit(s.failures, channel),
        worldVersion: s.worldVersion + 1,
      };
    }
    case "restore":
      return { ...s, restore: a.restore };
    case "sources":
      return { ...s, sourcesOpen: s.sourcesOpen[a.message] ? omit(s.sourcesOpen, a.message) : { ...s.sourcesOpen, [a.message]: true } };
    case "awaiting":
      // Sending starts afresh: last time's failure no longer applies.
      return a.on
        ? { ...s, awaiting: { ...s.awaiting, [a.channel]: true }, failures: omit(s.failures, a.channel) }
        : { ...s, awaiting: answered(s, a.channel) };
    case "loading":
      return { ...s, loading: { ...s.loading, [a.key]: true } };
    case "loaded":
      return {
        ...s,
        loading: omit(s.loading, a.key),
        loadErrors: a.error ? { ...s.loadErrors, [a.key]: a.error } : omit(s.loadErrors, a.key),
      };
    case "open":
      return {
        ...s,
        channel: a.channel,
        drawer: null,
        menuOpen: false,
        find: null,
        temp: s.temp && s.temp.id === a.channel ? s.temp : null,
        unread: omit(s.unread, a.channel ?? s.hostChannel ?? ""),
      };
    case "userAvatar":
      return { ...s, userAvatar: a.avatar };
    case "temp":
      return { ...s, temp: a.row };
    case "prefs":
      return { ...s, pinned: a.pinned, theme: a.theme };
    case "pin":
      return { ...s, pinned: a.on ? [...s.pinned.filter((c) => c !== a.channel), a.channel] : s.pinned.filter((c) => c !== a.channel) };
    case "theme":
      return { ...s, theme: a.theme };
    case "listView":
      return { ...s, listView: a.view, menuOpen: false };
    case "menu":
      return { ...s, menuOpen: a.open };
    case "find":
      return { ...s, find: a.query };
    case "profile":
      return { ...s, profile: a.mode, menuOpen: false };
    case "unread":
      // Read: the saved count goes too, or the badge would come back with the list.
      return a.count > 0
        ? { ...s, unread: { ...s.unread, [a.channel]: a.count } }
        : { ...s, unread: omit(s.unread, a.channel), channels: s.channels.map((c) => (c.id === a.channel && c.unread > 0 ? { ...c, unread: 0 } : c)) };
    case "drawer":
      return { ...s, drawer: a.drawer };
    case "ask":
      return { ...s, ask: a.draft };
    case "groupDialog":
      return { ...s, groupDialog: a.open };
    case "dismissLibraryNotice":
      return { ...s, libraryNoticeDismissed: true };
    case "error":
      return { ...s, error: a.error };
  }
}
