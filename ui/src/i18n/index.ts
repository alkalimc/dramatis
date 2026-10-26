import type { Wording } from "../api";
import { useStore } from "../store";
import { en, type Key } from "./en";

export type { Key };
export type Vars = Record<string, string | number>;
export type T = (key: Key, vars?: Vars) => string;

/** The corpus wording wins; the neutral English default fills every gap. */
export function translate(entries: Record<string, string>, key: Key, vars?: Vars): string {
  const template = entries[key] ?? en[key];
  if (!vars) return template;
  return template.replace(/\{(\w+)\}/g, (whole, name: string) => (name in vars ? String(vars[name]) : whole));
}

export function makeT(wording: Pick<Wording, "entries">): T {
  // Names any template may lean on, so a sentence never hard-codes the corpus's nouns.
  const globals: Vars = {
    host: translate(wording.entries, "host.name"),
    org: translate(wording.entries, "org.name"),
    person: translate(wording.entries, "person.noun"),
    people: translate(wording.entries, "person.noun_plural"),
  };
  return (key, vars) => translate(wording.entries, key, { ...globals, ...vars });
}

/**
 * How the user is addressed: the title around their name, or the corpus's word for "you"
 * while no name is set. The name is the user's own text, shown as typed.
 */
export function userTitle(t: T, name: string | null | undefined): string {
  const n = name?.trim();
  return n ? t("user.title", { name: n }) : t("user.you");
}

export function useT(): T {
  return useStore((s) => s.t);
}

/** The user's title from settings; follows renames at once. */
export function useUserTitle(): string {
  const t = useT();
  const name = useStore((s) => s.settings?.user_name ?? null);
  return userTitle(t, name);
}
