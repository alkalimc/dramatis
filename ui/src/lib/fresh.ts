// The save made by the new-save flow and the one the user came from, kept for this window
// (it survives the reload that opens a save) so the first-run steps can undo the flow.
const KEY = "ui.fresh";

export interface Fresh {
  name: string;
  /** The save that was open when the flow began; null when none was (the picker). */
  from: string | null;
}

export function markFresh(f: Fresh) {
  try {
    sessionStorage.setItem(KEY, JSON.stringify(f));
  } catch {
    // Without storage the first step simply has no way back.
  }
}

export function readFresh(): Fresh | null {
  try {
    const raw = JSON.parse(sessionStorage.getItem(KEY) ?? "null") as Partial<Fresh> | null;
    return raw && typeof raw.name === "string" ? { name: raw.name, from: typeof raw.from === "string" ? raw.from : null } : null;
  } catch {
    return null;
  }
}

export function clearFresh() {
  try {
    sessionStorage.removeItem(KEY);
  } catch {
    // Nothing was stored.
  }
}
