// Mock side of `ext.ts`: the user's picture, channel names, avatars and the archive flag,
// kept in memory and laid over the mock's channel list. Anything the base mock already
// implements wins, so this fades out once `mock/index.ts` has the commands.
import type { ChannelSummary, ImagePart } from "../../api.gen";
import type { Commands } from "..";
import type { ExtCommands } from "../ext";

const ok = <T>(data: T) => Promise.resolve({ status: "ok", data } as const);
const url = (i: ImagePart) => `data:${i.mime};base64,${i.data_base64}`;

/** A flat coloured badge with a letter: stands in for the default picture. */
function defaultPicture(): string {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect width="64" height="64" fill="#3b7f9c"/><circle cx="32" cy="25" r="11" fill="#d9eaf2"/><path d="M10 60c2-14 12-20 22-20s20 6 22 20z" fill="#d9eaf2"/></svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

export function withExtras<C extends Commands>(base: C): C & ExtCommands {
  if ("renameChannel" in base) return base as C & ExtCommands;
  let me: string | null = null;
  const over = new Map<string, { name?: string | null; avatar?: { path: string } | null; archived?: boolean }>();
  const set = (channel: string, patch: object) => over.set(channel, { ...over.get(channel), ...patch });
  const extra: ExtCommands = {
    userAvatar: () => ok({ path: me ?? defaultPicture() }),
    setUserAvatar: (image) => ((me = image ? url(image) : null), ok(null)),
    renameChannel: (channel, name) => (set(channel, { name: name?.trim() || null }), ok(null)),
    setChannelAvatar: (channel, image) => (set(channel, { avatar: image ? { path: url(image) } : null }), ok(null)),
    archiveChannel: (channel, archived) => (set(channel, { archived }), ok(null)),
  };
  return {
    ...base,
    ...extra,
    channels: async () => {
      const r = await base.channels();
      if (r.status !== "ok") return r;
      return { status: "ok", data: r.data.map((c): ChannelSummary => ({ ...c, ...over.get(c.id) }) as ChannelSummary) };
    },
  };
}
