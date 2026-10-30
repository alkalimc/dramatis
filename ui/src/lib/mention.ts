/** An `@` the user is typing in a composer: where it starts and what follows it so far. */
export interface Mention {
  start: number;
  query: string;
}

/** The mention being typed just before the caret: an `@` at the start of a word, and no space since. */
export function mentionAt(text: string, caret: number): Mention | null {
  const head = text.slice(0, caret);
  const m = /(^|\s)@([^\s@]*)$/u.exec(head);
  if (!m) return null;
  return { start: head.length - m[2].length - 1, query: m[2] };
}

/** `text` with the mention being typed replaced by `@name` and a space; the caret goes after the space. */
export function insertMention(text: string, at: Mention, caret: number, name: string): { text: string; caret: number } {
  const rest = text.slice(caret);
  // One space after the name: the one already there, else a new one.
  const gap = /^\s/.test(rest) ? "" : " ";
  const put = `@${name}${gap}`;
  return { text: text.slice(0, at.start) + put + rest, caret: at.start + put.length + (gap ? 0 : 1) };
}

/** Those of `names` that start with, or contain, what was typed after the `@`; starts first. */
export function filterMembers<T extends { name: string }>(members: T[], query: string): T[] {
  const q = query.trim().toLowerCase();
  if (!q) return members;
  const starts = members.filter((m) => m.name.toLowerCase().startsWith(q));
  const inside = members.filter((m) => !m.name.toLowerCase().startsWith(q) && m.name.toLowerCase().includes(q));
  return [...starts, ...inside];
}

const escapeHtml = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
const escapeRe = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** Wrap each `@name` of the given members in rendered message HTML; tags and attributes are left alone. */
export function highlightMentions(html: string, names: string[]): string {
  const wanted = [...new Set(names.map((n) => n.trim()).filter(Boolean))].sort((a, b) => b.length - a.length);
  if (wanted.length === 0 || !html.includes("@")) return html;
  const re = new RegExp(`@(?:${wanted.map((n) => escapeRe(escapeHtml(n))).join("|")})`, "gu");
  return html
    .split(/(<[^>]*>)/)
    .map((part) => (part.startsWith("<") ? part : part.replace(re, (hit) => `<span class="mention">${hit}</span>`)))
    .join("");
}
