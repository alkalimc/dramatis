import type { ChannelSummary, Message } from "../api";
import { makeT } from "../i18n";
import { byActivity, channelTitle, groupTitle, previewOf, unreadOf } from "./chats";

function channel(id: string, kind: ChannelSummary["kind"], last_at: string | null): ChannelSummary {
  return { id, kind, origin: "user", participants: [], topic: null, mode: "enabled", last_at, name: null, avatar: null, archived: false, unread: 0 };
}

function message(id: string, channel: string, at: string, text = "hi"): Message {
  return {
    id,
    channel,
    author: { kind: "person", id: "p1" },
    text,
    actions: [],
    attachment: null,
    picture_lost: false,
    cites: [],
    confidence: null,
   
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

test("the badge is the saved count, or the one kept here since, whichever is more", () => {
  const saved = { id: "c1", unread: 2 };
  expect(unreadOf(saved, {})).toBe(2);
  expect(unreadOf(saved, { c1: 5 })).toBe(5);
  expect(unreadOf({ id: "c2", unread: 0 }, { c2: 1 })).toBe(1);
  expect(unreadOf({ id: "c3", unread: 0 }, {})).toBe(0);
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

  test("the user is named first, then up to two members, and the rest are counted", () => {
    expect(groupTitle(group(1), t, "Me")).toBe("Me, A");
    expect(groupTitle(group(2), t, "Me")).toBe("Me, A, B");
    expect(groupTitle(group(3), t, "Me")).toBe("Me, A (4 in all)");
    expect(groupTitle(group(5), t, "Me")).toBe("Me, A (6 in all)");
  });

  test("the pack's separator and wording apply", () => {
    const pack = makeT({ entries: { "group.name.separator": " / ", "group.name.more": "{names} (+{count})" } });
    expect(groupTitle(group(2), pack, "Me")).toBe("Me / A / B");
    expect(groupTitle(group(5), pack, "Me")).toBe("Me / A (+6)");
  });

  test("a name the user gave wins; an empty one does not", () => {
    expect(groupTitle(group(3, { name: " Night shift " }), t, "Me")).toBe("Night shift");
    expect(groupTitle(group(2, { name: "  " }), t, "Me")).toBe("Me, A, B");
  });

  test("direct chats and the host are named for who they are", () => {
    expect(channelTitle({ ...channel("d", "direct", null), participants: people(1) }, t, "Me")).toBe("A");
    expect(channelTitle(channel("h", "host", null), t, "Me")).toBe("Host");
  });
});
