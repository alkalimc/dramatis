/**
 * Whether a person can speak yet. The engine adds `has_persona` to roster rows and person
 * pages; until it does (or while it is absent), everyone counts as available.
 */
export function hasPersona(view: object): boolean {
  const v = (view as { has_persona?: unknown }).has_persona;
  return typeof v === "boolean" ? v : true;
}
