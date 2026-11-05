/**
 * Window widths (px) at which the frame gives way, widest first. `layout.css` repeats
 * the numbers in its media queries (`shell.test.ts` checks they agree). Shrinking from
 * full screen, the right panel closes first and the chat list folds to pictures after.
 */
export const PANEL_CLOSES = 1100;
export const LIST_FOLDS = 900;
