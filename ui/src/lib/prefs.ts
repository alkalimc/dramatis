import type { ChannelId } from "../api";
import type { Theme } from "../store";

// Window-local choices: they belong to this screen, not to the world, so they stay out of
// the engine's settings.
const KEY = "ui.prefs";

export interface Prefs {
  pinned: ChannelId[];
  theme: Theme;
}

export function loadPrefs(): Prefs {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? "{}") as Partial<Prefs>;
    const theme = raw.theme === "light" || raw.theme === "dark" ? raw.theme : "system";
    return { pinned: Array.isArray(raw.pinned) ? raw.pinned.filter((c) => typeof c === "string") : [], theme };
  } catch {
    return { pinned: [], theme: "system" };
  }
}

export function savePrefs(p: Prefs) {
  try {
    localStorage.setItem(KEY, JSON.stringify(p));
  } catch {
    // Private mode or a full disk: the choice lasts until the window closes.
  }
}

/** Whether the screen is dark right now, given the stored choice and the system setting. */
export function isDark(theme: Theme): boolean {
  if (theme !== "system") return theme === "dark";
  return typeof matchMedia === "function" ? !matchMedia("(prefers-color-scheme: light)").matches : true;
}
