/**
 * How the frame gives way when the window shrinks, widest first. `layout.css` repeats the
 * numbers in its queries (`shell.test.ts` checks they agree).
 *
 * 1. The right panel judges by the box beside the chat list (`.body`), not by the window:
 *    it docks only while that box holds the conversation at its least width plus the
 *    panel. Narrower, it is closed (the `narrow` action) and opens as an overlay.
 * 2. Only then, at `LIST_FOLDS` window pixels, the chat list folds to pictures.
 * 3. The conversation keeps its width: its least width is never taken by the panel.
 *
 * Rems are the root's 15px, in the stylesheet and here alike.
 */
export const CHAT_MIN_REM = 34;
export const PANEL_REM = 22;
export const DOCK_MIN_REM = CHAT_MIN_REM + PANEL_REM;
/** Below this many window pixels the chat list shows pictures only. */
export const LIST_FOLDS = 900;
