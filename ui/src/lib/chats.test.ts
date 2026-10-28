import type { ChannelSummary, Message } from "../api";
import { makeT } from "../i18n";
import { byActivity, channelTitle, groupTitle, previewOf, unreadOf } from "./chats";

function channel(id: string, kind: ChannelSummary["kind"], last_at: string | null): ChannelSummary {
  return { id, kind, origin: "user", participants: [], topic: null, mode: "enabled", last_at, name: null, avatar: null, archived: false };
}

function message(id: string, channel: string, at: string, text = "hi"): Message {
  return {
    id,
    channel,
    author: { kind: "person", id: "p1" },
    text,
    actions: [],
    attachment: null,
    cites: [],
    confidence: null,
    relayed: false,
    task: null,
    note: null,
    at,
  };
}

test("chats are ordered by last activity, newest first, and a loaded message can be newer than the stamp", () => {
  const list = [
    channel("old", "direct", "2026-01-01T10:00:00Z"),
    channel("new", "group", "2026-01-03T10:00:00Z"),
    channel("quiet", "direct", null),
    channel("host", "host", null),
    channel("stale", "direct", "2026-01-02T10:00:00Z"),
  ];
  const loaded = { stale: [message("m", "stale", "2026-01-04T10:00:00Z")] };
  expect(byActivity(list, loaded).map((c) => c.id)).toEqual(["stale", "new", "old", "host", "quiet"]);
});

test("the badge counts unseen messages or waiting people, whichever is more", () => {
  const person = { id: "p1", name: "Ann" };
  const digest = {
    items: [
      { person, channel: "c1", activity: { kind: "birthday" } as const },
      { person, channel: "c1", activity: { kind: "came_by", at: "2026-01-01T10:00:00Z" } as const },
    ],
  };
  expect(unreadOf("c1", {}, digest)).toBe(2);
  expect(unreadOf("c1", { c1: 5 }, digest)).toBe(5);
  expect(unreadOf("c2", { c2: 1 }, digest)).toBe(1);
  expect(unreadOf("c3", {}, digest)).toBe(0);
});

test("the preview is the speech on one line, without citation marks; a scene-only reply shows the scene", () => {
  const star = { open: "*", close: "*" };
  expect(previewOf(message("a", "c", "2026-01-01T10:00:00Z", "*Rain.*\nFirst line.[#c-1]\nSecond line."), star)).toBe("First line. Second line.");
  expect(previewOf(message("b", "c", "2026-01-01T10:00:00Z", "*She waves.*"), star)).toBe("She waves.");
});

describe("group titles", () => {
  const t = makeT({ entries: {} });
  const people = (n: number) => Array.from({ length: n }, (_, i) => ({ id: `p${i}`, name: String.fromCharCode(65 + i) }));
  const group = (n: number, extra: object = {}): ChannelSummary => ({ ...channel("g", "group", null), participants: people(n), ...extra });

  test("up to three members are all named, then the rest are counted", () => {
    expect(groupTitle(group(1), t)).toBe("A");
    expect(groupTitle(group(3), t)).toBe("A, B, C");
    expect(groupTitle(group(5), t)).toBe("A, B (5 in all)");
  });

  test("the pack's separator and wording apply", () => {
    const pack = makeT({ entries: { "group.name.separator": " / ", "group.name.more": "{names} (+{count})" } });
    expect(groupTitle(group(3), pack)).toBe("A / B / C");
    expect(groupTitle(group(5), pack)).toBe("A / B (+5)");
  });

  test("a name the user gave wins; an empty one does not", () => {
    expect(groupTitle(group(3, { name: " Night shift " }), t)).toBe("Night shift");
    expect(groupTitle(group(3, { name: "  " }), t)).toBe("A, B, C");
  });

  test("direct chats and the host are named for who they are", () => {
    expect(channelTitle({ ...channel("d", "direct", null), participants: people(1) }, t)).toBe("A");
    expect(channelTitle(channel("h", "host", null), t)).toBe("Host");
  });
});
