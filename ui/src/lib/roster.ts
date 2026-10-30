import type { Mode, RosterRow } from "../api";

/** The roster's quick filters; "all" and "any" mean the filter is off. */
export interface RosterFilter {
  mode: Mode | "all";
  talk: "any" | "talked" | "never";
  persona: boolean;
}

export const NO_FILTER: RosterFilter = { mode: "all", talk: "any", persona: false };

export function filtering(f: RosterFilter): boolean {
  return f.mode !== "all" || f.talk !== "any" || f.persona;
}

export function passes(row: Pick<RosterRow, "mode" | "talked" | "has_persona">, f: RosterFilter): boolean {
  return (f.mode === "all" || row.mode === f.mode) && (f.talk === "any" || row.talked === (f.talk === "talked")) && (!f.persona || row.has_persona);
}
