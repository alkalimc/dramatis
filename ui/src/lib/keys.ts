import type { KeyboardEvent } from "react";

/** Whether this is a Mac, where the send key is Cmd+Enter; elsewhere Ctrl+Enter. */
export function isMac(): boolean {
  return typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);
}

/**
 * The send shortcut of a message box: Cmd+Enter or Ctrl+Enter. A bare Enter is a new line,
 * and nothing counts while an input method is composing: the Enter that commits a
 * candidate must not send (keyCode 229 is how some engines report that Enter).
 */
export function isSendKey(e: KeyboardEvent): boolean {
  if (e.key !== "Enter" || e.nativeEvent.isComposing || e.keyCode === 229) return false;
  return e.metaKey || e.ctrlKey;
}
