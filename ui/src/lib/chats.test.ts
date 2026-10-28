import type { ChannelSummary, Message } from "../api";
import { byActivity, previewOf, unreadOf } from "./chats";

function channel(id: string, kind: ChannelSummary["kind"], last_at: string | null): ChannelSummary {
  return { id, kind, origin: "user", participants: [], topic: null, mode: "enabled", last_at };
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
