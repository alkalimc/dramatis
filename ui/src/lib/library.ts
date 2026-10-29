import type { State } from "../store/state";

/** Whether the app is in the library: chosen by the user, or no model to talk with. */
export const inLibrary = (s: State): boolean => s.library || (s.status !== null && s.status.form !== "full");
