import type { KeyboardEvent } from "react";

/** Whether this is a Mac, where the send key is Cmd+Enter; elsewhere Ctrl+Enter. */
export function isMac(): boolean {
  return typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);
}

/** Which Enter sends a message: a bare Enter, or Cmd+Enter / Ctrl+Enter. The other one adds a line. */
export type SendKey = "enter" | "mod";

const composing = (e: KeyboardEvent) => e.nativeEvent.isComposing || e.keyCode === 229;

/**
 * The send shortcut of a message box. With `mod` (the default) it is Cmd+Enter or Ctrl+Enter
 * and a bare Enter is a new line; with `enter` it is a bare Enter (Shift and Alt make it a
 * new line) and Cmd/Ctrl+Enter is the new line. Nothing counts while an input method is
 * composing: the Enter that commits a candidate must not send (keyCode 229 is how some
 * engines report that Enter).
 */
export function isSendKey(e: KeyboardEvent, mode: SendKey = "mod"): boolean {
  if (e.key !== "Enter" || composing(e)) return false;
  const mod = e.metaKey || e.ctrlKey;
  return mode === "mod" ? mod : !mod && !e.shiftKey && !e.altKey;
}

/**
 * An Enter the box must turn into a line break itself: Cmd/Ctrl+Enter when a bare Enter
 * sends. (The other combinations insert one on their own.)
 */
export function isNewlineKey(e: KeyboardEvent, mode: SendKey = "mod"): boolean {
  return mode === "enter" && e.key === "Enter" && !composing(e) && (e.metaKey || e.ctrlKey);
}

/**
 * Put a line break at the caret of a text box, keeping the browser's undo history where it
 * can. Otherwise `set` gets the new text and where the caret goes, for the caller to place
 * when the text is in (a later frame would be too late for keys typed meanwhile).
 */
export function insertNewline(box: HTMLTextAreaElement, set: (value: string, caret: number) => void) {
  if (typeof document.execCommand === "function" && document.execCommand("insertText", false, "\n")) return;
  const { selectionStart: a, selectionEnd: b, value } = box;
  set(`${value.slice(0, a)}\n${value.slice(b)}`, a + 1);
}
