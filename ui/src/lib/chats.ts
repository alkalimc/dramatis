import type { ChannelId, ChannelSummary, Digest, Message, SceneMarker } from "../api";
import { splitScene, withoutCites } from "./scene";

/** When a channel last saw something: the newer of the engine's stamp and the loaded log. */
export function lastActivity(c: ChannelSummary, loaded: Message[] | undefined): string | null {
  const tail = loaded?.[loaded.length - 1]?.at ?? null;
  if (!tail || !c.last_at) return tail ?? c.last_at;
  return Date.parse(tail) > Date.parse(c.last_at) ? tail : c.last_at;
}

/** Newest activity first; channels with none go last, the host ahead of the rest on a tie. */
export function byActivity(channels: ChannelSummary[], messages: Record<ChannelId, Message[]>): ChannelSummary[] {
  const stamp = (c: ChannelSummary) => {
    const at = lastActivity(c, messages[c.id]);
    return at ? Date.parse(at) : Number.NEGATIVE_INFINITY;
  };
  return channels
    .map((c, i) => ({ c, i, at: stamp(c) }))
    .sort((a, b) => (b.at === a.at ? Number(b.c.kind === "host") - Number(a.c.kind === "host") || a.i - b.i : b.at > a.at ? 1 : -1))
    .map((x) => x.c);
}

/** What the badge counts: messages that arrived unseen, or people waiting here, whichever is more. */
export function unreadOf(channel: ChannelId, unread: Record<ChannelId, number>, digest: Digest): number {
  const waiting = digest.items.filter((w) => w.channel === channel).length;
  return Math.max(unread[channel] ?? 0, waiting);
}

/** One line for the list: the speech, else the scene, with citation marks stripped. */
export function previewOf(m: Message, marker: SceneMarker): string {
  const segs = splitScene(m.text, marker);
  const pick = (kind: "speech" | "scene") => segs.filter((s) => s.kind === kind).map((s) => s.text).join(" ");
  const speech = withoutCites(pick("speech"));
  return (speech.trim() ? speech : pick("scene")).replace(/\s+/g, " ").trim();
}
