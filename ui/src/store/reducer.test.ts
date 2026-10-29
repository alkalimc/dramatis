import type { ChannelSummary, Message, Roster } from "../api";
import { reduce } from "./reducer";
import { initialState, type State } from "./state";

function message(id: string, at: string, extra: Partial<Message> = {}): Message {
  return {
    id,
    channel: "c1",
    author: { kind: "person", id: "p1" },
    text: id,
    actions: [],
    attachment: null,
    cites: [],
    confidence: null,
    relayed: false,
    task: null,
    note: null,
    at,
    ...extra,
  };
}

const roster: Roster = {
  total: 2,
  enabled: 0,
  groups: [
    {
      band: "close",
      rows: [
        { person: { id: "p1", name: "Ann" }, mode: "frozen", recent: null, companions: [{ id: "p2", name: "Bo" }], has_persona: true },
        { person: { id: "p2", name: "Bo" }, mode: "frozen", recent: null, companions: [], has_persona: true },
      ],
    },
  ],
};

test("streamed deltas accumulate, then the stored message replaces the stream", () => {
  let s: State = initialState();
  const author = { kind: "person", id: "p1" } as const;
  s = reduce(s, { type: "messageDelta", event: { channel: "c1", message: "m1", author, delta: "Hel" } });
  s = reduce(s, { type: "messageDelta", event: { channel: "c1", message: "m1", author, delta: "lo" } });
  expect(s.streams.m1.text).toBe("Hello");
  s = reduce(s, { type: "messageAdded", event: { message: message("m1", "2026-01-01T00:00:01Z", { text: "Hello" }) } });
  expect(s.streams.m1).toBeUndefined();
  expect(s.messages.c1.map((m) => m.text)).toEqual(["Hello"]);
});

test("tool calls before the message is stored are attached when it arrives", () => {
  let s = initialState();
  const author = { kind: "person", id: "p1" } as const;
  s = reduce(s, { type: "toolCalled", event: { channel: "c1", message: "m1", author, action: { tool: "search", query: "q" } } });
  expect(s.pendingActions.m1).toHaveLength(1);
  s = reduce(s, { type: "messageAdded", event: { message: message("m1", "2026-01-01T00:00:01Z") } });
  expect(s.messages.c1[0].actions).toEqual([{ tool: "search", query: "q" }]);
  expect(s.pendingActions.m1).toBeUndefined();
});

test("tool calls on a stored message append to it", () => {
  let s = reduce(initialState(), { type: "history", channel: "c1", messages: [message("m1", "2026-01-01T00:00:01Z")], older: false, done: true });
  s = reduce(s, { type: "toolCalled", event: { channel: "c1", message: "m1", author: { kind: "host" }, action: { tool: "digest" } } });
  expect(s.messages.c1[0].actions).toEqual([{ tool: "digest" }]);
});

test("history pages merge without duplicates, older pages in front", () => {
  let s = reduce(initialState(), {
    type: "history",
    channel: "c1",
    messages: [message("m2", "2026-01-01T00:00:02Z"), message("m3", "2026-01-01T00:00:03Z")],
    older: false,
    done: false,
  });
  s = reduce(s, { type: "history", channel: "c1", messages: [message("m1", "2026-01-01T00:00:01Z"), message("m2", "2026-01-01T00:00:02Z")], older: true, done: true });
  expect(s.messages.c1.map((m) => m.id)).toEqual(["m1", "m2", "m3"]);
  expect(s.historyDone.c1).toBe(true);
});

test("a late event lands in time order; a repeated one replaces", () => {
  let s = reduce(initialState(), { type: "messageAdded", event: { message: message("m2", "2026-01-01T00:00:02Z") } });
  s = reduce(s, { type: "messageAdded", event: { message: message("m1", "2026-01-01T00:00:01Z") } });
  s = reduce(s, { type: "messageAdded", event: { message: message("m2", "2026-01-01T00:00:02Z", { text: "edited" }) } });
  expect(s.messages.c1.map((m) => [m.id, m.text])).toEqual([
    ["m1", "m1"],
    ["m2", "edited"],
  ]);
});

test("mode changes update the roster row, the enabled count, channels and bump people", () => {
  let s = reduce(initialState(), { type: "roster", roster });
  s = reduce(s, {
    type: "channels",
    channels: [{ id: "g1", kind: "group", origin: "user", participants: [], topic: null, mode: "frozen", last_at: null, name: null, avatar: null, archived: false, unread: 0 }],
  });
  const v = s.peopleVersion;
  s = reduce(s, { type: "modeChanged", event: { target: { kind: "person", id: "p2" }, mode: "enabled" } });
  expect(s.roster!.groups[0].rows[1].mode).toBe("enabled");
  expect(s.roster!.enabled).toBe(1);
  s = reduce(s, { type: "modeChanged", event: { target: { kind: "channel", id: "g1" }, mode: "enabled" } });
  expect(s.channels[0].mode).toBe("enabled");
  expect(s.peopleVersion).toBe(v + 2);
});

test("names are collected from every payload that carries people", () => {
  const s = reduce(initialState(), { type: "roster", roster });
  expect(s.names).toEqual({ p1: "Ann", p2: "Bo" });
});

test("wording replaces the translator; missing keys fall back to English", () => {
  const s = reduce(initialState(), {
    type: "wording",
    wording: { entries: { "host.name": "Keeper" }, scene_marker: { open: "(", close: ")" } },
  });
  expect(s.t("host.name")).toBe("Keeper");
  expect(s.t("quota.shape.host")).toBe("Keeper");
  expect(s.t("nav.roster")).toBe("Roster");
  expect(s.wording.scene_marker.open).toBe("(");
});

test("world changes bump the version screens refetch on", () => {
  const s = reduce(initialState(), { type: "worldChanged", event: { changes: [{ kind: "digest" }] } });
  expect(s.worldVersion).toBe(1);
});

test("opening a channel closes the drawer", () => {
  let s = reduce(initialState(), { type: "drawer", drawer: { kind: "world" } });
  s = reduce(s, { type: "open", channel: "c1" });
  expect(s.drawer).toBeNull();
  expect(s.channel).toBe("c1");
});

test("activity tracks who is generating per channel and clears on done or failed", () => {
  let s: State = initialState();
  const ann = { kind: "person", id: "p1" } as const;
  const bo = { kind: "person", id: "p2" } as const;
  const send = (channel: string, author: typeof ann | typeof bo, state: import("../api").ActivityState) =>
    (s = reduce(s, { type: "turnActivity", event: { channel, author, state } }));
  send("c1", ann, { kind: "thinking" });
  send("c2", bo, { kind: "thinking" });
  expect(Object.keys(s.typing)).toEqual(["c1", "c2"]);
  send("c1", ann, { kind: "tool", name: "search" });
  expect(s.typing.c1.p1.tool).toBe(true);
  send("c1", ann, { kind: "writing" });
  expect(s.typing.c1.p1.tool).toBe(false);
  send("c1", ann, { kind: "done" });
  expect(s.typing.c1).toBeUndefined();
  expect(s.typing.c2.p2).toBeDefined();
  send("c2", bo, { kind: "failed", reason: "error" });
  expect(s.typing).toEqual({});
  // Clearing what was never set changes nothing.
  const same = s;
  send("c3", ann, { kind: "done" });
  expect(s).toBe(same);
});

test("activity ends the user's wait for a first sign", () => {
  let s: State = initialState();
  s = reduce(s, { type: "awaiting", channel: "c1", on: true });
  s = reduce(s, { type: "turnActivity", event: { channel: "c1", author: { kind: "host" }, state: { kind: "thinking" } } });
  expect(s.awaiting.c1).toBeUndefined();
  expect(s.typing.c1.host).toBeDefined();
});

test("a failed turn drops its half-written bubble", () => {
  const author = { kind: "person", id: "p1" } as const;
  let s = reduce(initialState(), { type: "messageDelta", event: { channel: "c1", message: "m1", author, delta: "Hal" } });
  expect(s.streams.m1.text).toBe("Hal");
  s = reduce(s, { type: "turnActivity", event: { channel: "c1", author, state: { kind: "failed", reason: "error" } } });
  expect(s.streams.m1).toBeUndefined();
});

test("a take-back drops that message and everything after it, and only in its channel", () => {
  let s: State = initialState();
  s = reduce(s, {
    type: "history",
    channel: "c1",
    messages: [message("m1", "2026-01-01T10:00:00Z"), message("m2", "2026-01-01T10:01:00Z"), message("m3", "2026-01-01T10:02:00Z")],
    older: false,
    done: true,
  });
  s = reduce(s, { type: "history", channel: "c2", messages: [message("n1", "2026-01-01T10:00:00Z", { channel: "c2" })], older: false, done: true });
  s = reduce(s, { type: "sources", message: "m3" });
  s = reduce(s, { type: "awaiting", channel: "c1", on: true });
  const version = s.worldVersion;
  s = reduce(s, { type: "rewound", event: { channel: "c1", from: "m2" } });
  expect(s.messages.c1.map((m) => m.id)).toEqual(["m1"]);
  expect(s.messages.c2.map((m) => m.id)).toEqual(["n1"]);
  expect(s.sourcesOpen).toEqual({});
  expect(s.awaiting).toEqual({});
  expect(s.worldVersion).toBe(version + 1);
  // Repeated or for a message that is not loaded: nothing more to drop.
  expect(reduce(s, { type: "rewound", event: { channel: "c1", from: "m2" } })).toBe(s);
});

test("sources start folded and toggle per message", () => {
  let s: State = initialState();
  expect(s.sourcesOpen.m1).toBeUndefined();
  s = reduce(s, { type: "sources", message: "m1" });
  expect(s.sourcesOpen.m1).toBe(true);
  s = reduce(s, { type: "sources", message: "m2" });
  s = reduce(s, { type: "sources", message: "m1" });
  expect(s.sourcesOpen).toEqual({ m2: true });
});

test("a failed turn is remembered per channel and cleared by the next send or activity", () => {
  const author = { kind: "person", id: "p1" } as const;
  let s = reduce(initialState(), { type: "turnActivity", event: { channel: "c1", author, state: { kind: "failed", reason: "quota_exhausted" } } });
  expect(s.failures.c1).toEqual({ author, reason: "quota_exhausted" });
  expect(s.failures.c2).toBeUndefined();
  const sent = reduce(s, { type: "awaiting", channel: "c1", on: true });
  expect(sent.failures.c1).toBeUndefined();
  const again = reduce(s, { type: "turnActivity", event: { channel: "c1", author, state: { kind: "thinking" } } });
  expect(again.failures.c1).toBeUndefined();
});

test("a turn that ends, done or failed, ends the user's wait too", () => {
  const author = { kind: "host" } as const;
  for (const state of [{ kind: "done" }, { kind: "failed", reason: "error" }] as const) {
    let s = reduce(initialState(), { type: "awaiting", channel: "c1", on: true });
    s = reduce(s, { type: "turnActivity", event: { channel: "c1", author, state } });
    expect(s.awaiting.c1).toBeUndefined();
  }
});

test("messages from others count as unread unless their channel is the one shown; opening clears", () => {
  let s: State = { ...initialState(), hostChannel: "host", channel: "c1" };
  s = reduce(s, { type: "messageAdded", event: { message: message("m1", "2026-01-01T10:00:00Z") } });
  expect(s.unread).toEqual({});
  s = reduce(s, { type: "messageAdded", event: { message: message("m2", "2026-01-01T10:01:00Z", { channel: "c2" }) } });
  s = reduce(s, { type: "messageAdded", event: { message: message("m3", "2026-01-01T10:02:00Z", { channel: "c2" }) } });
  s = reduce(s, { type: "messageAdded", event: { message: message("m4", "2026-01-01T10:03:00Z", { channel: "c2", author: { kind: "user" } }) } });
  expect(s.unread).toEqual({ c2: 2 });
  s = reduce(s, { type: "open", channel: "c2" });
  expect(s.unread).toEqual({});
});

test("a reload merges into what is shown: new messages join in time order, known ones are replaced", () => {
  let s = initialState();
  s = reduce(s, { type: "history", channel: "c1", messages: [message("a", "2026-01-01T10:00:00Z"), message("c", "2026-01-01T12:00:00Z")], older: false, done: true });
  s = reduce(s, {
    type: "history",
    channel: "c1",
    messages: [message("b", "2026-01-01T11:00:00Z"), message("c", "2026-01-01T12:00:00Z", { text: "edited" })],
    older: false,
    done: true,
    merge: true,
  });
  expect(s.messages.c1.map((m) => m.id)).toEqual(["a", "b", "c"]);
  expect(s.messages.c1[2].text).toBe("edited");
});

test("pinning, marking unread and the temporary row", () => {
  let s = initialState();
  s = reduce(s, { type: "pin", channel: "c1", on: true });
  s = reduce(s, { type: "pin", channel: "c2", on: true });
  s = reduce(s, { type: "pin", channel: "c1", on: false });
  expect(s.pinned).toEqual(["c2"]);

  s = reduce(s, { type: "unread", channel: "c1", count: 1 });
  expect(s.unread.c1).toBe(1);
  s = reduce(s, { type: "unread", channel: "c1", count: 0 });
  expect(s.unread.c1).toBeUndefined();

  const row = { id: "t1", kind: "direct", origin: "system", participants: [], topic: null, mode: "enabled", last_at: null, name: null, avatar: null, archived: false, unread: 0 } as const;
  s = reduce(s, { type: "temp", row: { ...row, participants: [] } });
  // Opening it keeps the row; opening anything else drops it.
  expect(reduce(s, { type: "open", channel: "t1" }).temp?.id).toBe("t1");
  expect(reduce(s, { type: "open", channel: "c9" }).temp).toBeNull();
});

test("marking read clears the saved count in the list with the local one", () => {
  const row = { id: "c1", kind: "direct", origin: "system", participants: [], topic: null, mode: "enabled", last_at: null, name: null, avatar: null, archived: false, unread: 3 } satisfies ChannelSummary;
  let s = reduce(initialState(), { type: "channels", channels: [row, { ...row, id: "c2" }] });
  s = reduce(s, { type: "unread", channel: "c1", count: 0 });
  expect(s.channels.map((c) => c.unread)).toEqual([0, 3]);
});
