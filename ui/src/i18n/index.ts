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
  return template.replace(/\{(\w+)\}/g, (whole, name: string) =>
    name in vars ? String(vars[name]) : whole,
  );
}

export function makeT(wording: Pick<Wording, "entries">): T {
  // Names other keys lean on: `{host}` in any template means the host's name.
  const host = translate(wording.entries, "host.name");
  return (key, vars) => translate(wording.entries, key, { host, ...vars });
}

export function useT(): T {
  return useStore((s) => s.t);
}
