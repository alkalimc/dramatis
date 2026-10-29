import type { State } from "../store/state";

/** Whether the app is in the library: chosen by the user, or no model to talk with. */
export const inLibrary = (s: State): boolean => s.library || (s.status !== null && s.status.form !== "full");

/** Whether a conversation can be started: a model exists, whether or not the library view is open. */
export const canTalk = (s: State): boolean => s.status === null || s.status.form === "full";
