import type { SceneMarker } from "../api";

export type Segment = { kind: "scene"; text: string } | { kind: "speech"; text: string };

/**
 * A whole line wrapped in the corpus's scene marker is scene description; every other
 * line is speech. Consecutive speech lines stay together (they may be one Markdown
 * paragraph). Only whole lines count: a marker inside a sentence is just text.
 */
export function splitScene(text: string, marker: SceneMarker): Segment[] {
  const out: Segment[] = [];
  let speech: string[] = [];
  const flush = () => {
    const joined = speech.join("\n").replace(/^\n+|\n+$/g, "");
    if (joined.trim()) out.push({ kind: "speech", text: joined });
    speech = [];
  };
  for (const line of text.split(/\r?\n/)) {
    const inner = sceneInner(line.trim(), marker);
    if (inner === null) {
      speech.push(line);
    } else {
      flush();
      out.push({ kind: "scene", text: inner });
    }
  }
  flush();
  return out;
}

/** The inside of a scene line, or `null` when the line is not one. */
function sceneInner(line: string, { open, close }: SceneMarker): string | null {
  if (!open || !close) return null;
  if (line.length <= open.length + close.length) return null;
  if (!line.startsWith(open) || !line.endsWith(close)) return null;
  const inner = line.slice(open.length, line.length - close.length);
  // `**bold**` with the default `*` marker is emphasis, not a scene line; likewise a
  // doubled custom marker.
  if (inner.startsWith(open) || inner.endsWith(close)) return null;
  if (!inner.trim()) return null;
  return inner.trim();
}

/** The speech of a message without its scene lines: what read-aloud would say. */
export function speechOnly(text: string, marker: SceneMarker): string {
  return splitScene(text, marker)
    .filter((s) => s.kind === "speech")
    .map((s) => s.text)
    .join("\n");
}
