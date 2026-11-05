import type { FactView } from "../api";

/** The groups the world page's recent cards sort memories into. */
export type Category = "world" | "said" | "own" | "commitment" | "hurt";

export const CATEGORIES: Category[] = ["world", "said", "own", "commitment", "hurt"];

/** Memories shown per category in the recent cards. */
export const RECENT = 3;

/** Which group a memory belongs to: its kind first (a promise, a hurt), else who can recall it. */
export function categoryOf(f: FactView): Category {
  if (f.kind === "commitment") return "commitment";
  if (f.kind === "hurt") return "hurt";
  if (f.audience.kind === "own") return "own";
  if (f.audience.kind === "world") return "world";
  return "said";
}

/** The newest live memories of each category, `per` of each, empty groups left out. */
export function recentByCategory(facts: FactView[], per: number = RECENT): { category: Category; items: FactView[] }[] {
  const live = facts.filter((f) => !f.retracted).sort((a, b) => b.created_at.localeCompare(a.created_at));
  return CATEGORIES.map((category) => ({ category, items: live.filter((f) => categoryOf(f) === category).slice(0, per) })).filter((g) => g.items.length > 0);
}
