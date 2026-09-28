import type { Message, Mode, PersonRef, Roster, Target } from "../api";
import { makeT } from "../i18n";
import type { Action, State } from "./state";

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

function omit<V>(rec: Record<string, V>, key: string): Record<string, V> {
  if (!(key in rec)) return rec;
  const { [key]: _drop, ...rest } = rec;
  return rest;
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
      const merged = a.older ? [...fresh, ...cur] : [...cur, ...fresh];
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
      return {
        ...s,
        pendingActions: { ...s.pendingActions, [e.message]: [...(s.pendingActions[e.message] ?? []), e.action] },
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
    case "open":
      return { ...s, channel: a.channel, drawer: null };
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
