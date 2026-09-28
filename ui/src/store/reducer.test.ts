import type { Message, Roster } from "../api";
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
        { person: { id: "p1", name: "Ann" }, mode: "frozen", recent: null, companions: [{ id: "p2", name: "Bo" }] },
        { person: { id: "p2", name: "Bo" }, mode: "frozen", recent: null, companions: [] },
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
    channels: [{ id: "g1", kind: "group", origin: "user", participants: [], topic: null, mode: "frozen", last_at: null }],
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
