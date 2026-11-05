import type { FactView } from "../api";
import { categoryOf, recentByCategory } from "./world";

const fact = (id: string, over: Partial<FactView>, minutesAgo = 0): FactView => ({
  id,
  author: { kind: "user" },
  audience: { kind: "world" },
  kind: "fact",
  about: null,
  due: null,
  delivered: false,
  outcome: null,
  text: id,
  retracted: false,
  created_at: new Date(Date.UTC(2026, 0, 1, 12, 0) - minutesAgo * 60_000).toISOString(),
  ...over,
});

test("a memory is a promise or a hurt by its kind, else by who can recall it", () => {
  expect(categoryOf(fact("a", { kind: "commitment" }))).toBe("commitment");
  expect(categoryOf(fact("a", { kind: "hurt", audience: { kind: "own", person: { id: "p", name: "P" } } }))).toBe("hurt");
  expect(categoryOf(fact("a", { audience: { kind: "own", person: { id: "p", name: "P" } } }))).toBe("own");
  expect(categoryOf(fact("a", { audience: { kind: "participants", channel: "c", persons: [] } }))).toBe("said");
  expect(categoryOf(fact("a", {}))).toBe("world");
});

test("the recent cards take the newest few of each category, not the newest overall", () => {
  const facts = [
    ...[0, 1, 2, 3, 4].map((i) => fact(`w${i}`, {}, i)),
    fact("promise", { kind: "commitment" }, 500),
    fact("gone", { kind: "commitment" }, 1),
  ];
  facts[6] = { ...facts[6], retracted: true };
  const groups = recentByCategory(facts, 3);
  expect(groups.map((g) => g.category)).toEqual(["world", "commitment"]);
  expect(groups[0].items.map((f) => f.id)).toEqual(["w0", "w1", "w2"]);
  // An old promise still shows: the world's newer memories do not push it out.
  expect(groups[1].items.map((f) => f.id)).toEqual(["promise"]);
});
