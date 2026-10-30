/** Scroll the open log to a message and flash it. False when it is not on screen. */
export function jumpTo(id: string): boolean {
  const el = document.getElementById(`m-${id}`);
  if (!el) return false;
  el.scrollIntoView?.({ block: "center" });
  el.classList.add("flash");
  setTimeout(() => el.classList.remove("flash"), 1600);
  return true;
}
