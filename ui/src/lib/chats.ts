import type { ChannelId, ChannelSummary, Digest, Message, SceneMarker } from "../api";
import type { T } from "../i18n";
import { splitScene, withoutCites } from "./scene";

/** Most member names a group's default title spells out before it counts everyone. */
const TITLE_NAMES = 3;

/** A group's own name, else its members: "A, B, C", or "A, B (5 in all)" once there are many. */
export function groupTitle(c: ChannelSummary, t: T): string {
  const own = c.name?.trim();
  if (own) return own;
  const names = c.participants.map((p) => p.name);
  const sep = t("group.name.separator");
  if (names.length === 0) return t("channel.group.untitled");
  if (names.length <= TITLE_NAMES) return names.join(sep);
  return t("group.name.more", { names: names.slice(0, 2).join(sep), count: names.length });
}

/** What a conversation is called in lists and headers. */
export function channelTitle(c: ChannelSummary, t: T): string {
  if (c.kind === "host") return t("host.name");
  if (c.kind === "direct") return c.participants[0]?.name ?? t("person.unknown");
  return groupTitle(c, t);
}

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

/**
 * What the badge counts: messages that arrived unseen, or people with something unread
 * here, whichever is more. A birthday or a due commitment is a reminder rather than an
 * unread message and nothing the user does in the chat settles it, so it never counts.
 */
export function unreadOf(channel: ChannelId, unread: Record<ChannelId, number>, digest: Digest): number {
  const waiting = digest.items.filter((w) => w.channel === channel && w.activity.kind !== "birthday" && w.activity.kind !== "commitment_due").length;
  return Math.max(unread[channel] ?? 0, waiting);
}

/** One line for the list: the speech, else the scene, with citation marks stripped. */
export function previewOf(m: Message, marker: SceneMarker): string {
  const segs = splitScene(m.text, marker);
  const pick = (kind: "speech" | "scene") => segs.filter((s) => s.kind === kind).map((s) => s.text).join(" ");
  const speech = withoutCites(pick("speech"));
  return (speech.trim() ? speech : pick("scene")).replace(/\s+/g, " ").trim();
}
