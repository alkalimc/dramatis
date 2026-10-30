/** The words a search was made of: split on spaces, longest first so a long word wins over its part. */
export function termsOf(query: string): string[] {
  return [...new Set(query.toLowerCase().split(/\s+/).filter(Boolean))].sort((a, b) => b.length - a.length);
}

/** A piece of `text` around the first hit of any term, cut at `radius` characters each side, with an ellipsis where it was cut. */
export function snippet(text: string, terms: string[], radius = 70): string {
  const flat = text.replace(/\s+/g, " ").trim();
  const low = flat.toLowerCase();
  const at = terms.reduce((best, w) => {
    const i = low.indexOf(w);
    return i >= 0 && (best < 0 || i < best) ? i : best;
  }, -1);
  if (at < 0 || flat.length <= radius * 2) return flat.length > radius * 2 ? `${flat.slice(0, radius * 2)}…` : flat;
  const from = Math.max(0, at - radius);
  const to = Math.min(flat.length, at + radius);
  return `${from > 0 ? "…" : ""}${flat.slice(from, to)}${to < flat.length ? "…" : ""}`;
}

/** `text` cut into pieces, each saying whether it is a hit of one of the terms. */
export function pieces(text: string, terms: string[]): { text: string; hit: boolean }[] {
  if (terms.length === 0) return [{ text, hit: false }];
  const re = new RegExp(terms.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|"), "gi");
  const out: { text: string; hit: boolean }[] = [];
  let last = 0;
  for (const m of text.matchAll(re)) {
    if (m[0] === "") continue;
    if (m.index > last) out.push({ text: text.slice(last, m.index), hit: false });
    out.push({ text: m[0], hit: true });
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push({ text: text.slice(last), hit: false });
  return out;
}
