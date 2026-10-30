import type { Message } from "../api";
import { deliveryOf } from "./ticks";

const msg = (id: string, kind: "user" | "person", service = false): Message => ({
  id,
  channel: "c",
  author: kind === "user" ? { kind: "user" } : { kind: "person", id: "p" },
  text: id,
  actions: [],
  attachment: null,
  picture_lost: false,
  cites: [],
  confidence: null,
  relayed: false,
  service,
  at: "2030-01-01T00:00:00Z",
});

test("no tick before the reply has started", () => {
  expect(deliveryOf([msg("u", "user")], "u", false, false)).toBe(0);
});

test("one tick while a reply is being written", () => {
  expect(deliveryOf([msg("u", "user")], "u", true, false)).toBe(1);
  expect(deliveryOf([msg("u", "user"), msg("a", "person")], "u", true, false)).toBe(1);
});

test("two ticks once the replies are in and nobody is writing", () => {
  expect(deliveryOf([msg("u", "user"), msg("a", "person")], "u", false, false)).toBe(2);
});

test("history: a message followed by someone else's is answered", () => {
  const list = [msg("u1", "user"), msg("a", "person"), msg("u2", "user")];
  expect(deliveryOf(list, "u1", false, false)).toBe(2);
  expect(deliveryOf(list, "u2", false, false)).toBe(0);
  // a new round under way does not take the old tick away
  expect(deliveryOf(list, "u1", true, false)).toBe(2);
});

test("a failed turn shows no second tick", () => {
  expect(deliveryOf([msg("u", "user")], "u", false, true)).toBe(0);
  expect(deliveryOf([msg("u", "user"), msg("a", "person")], "u", false, true)).toBe(1);
});

test("two messages sent in a row share the round", () => {
  const list = [msg("u1", "user"), msg("u2", "user"), msg("a", "person")];
  expect(deliveryOf(list, "u1", false, false)).toBe(2);
  expect(deliveryOf(list, "u2", false, false)).toBe(2);
});

test("a service line is not an answer", () => {
  expect(deliveryOf([msg("u", "user"), msg("s", "person", true)], "u", false, false)).toBe(0);
});
