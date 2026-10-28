// The commands the next engine revision adds for avatars, channel names and archiving.
// Until the generated bindings carry them, screens reach them through `ext(api)` and the
// mock supplies them (see `mock/extra.ts`). When `api.gen.ts` has them, drop this file and
// call `api.commands` directly.
import type { ChannelId, ChannelSummary, ImagePart, MediaFile } from "../api.gen";
import type { Api, Commands, Result } from ".";

export interface ExtCommands {
  /** The user's picture: the uploaded one, else the default; `null` only if neither exists. */
  userAvatar(): Result<MediaFile | null>;
  setUserAvatar(image: ImagePart | null): Result<null>;
  renameChannel(channel: ChannelId, name: string | null): Result<null>;
  setChannelAvatar(channel: ChannelId, image: ImagePart | null): Result<null>;
  archiveChannel(channel: ChannelId, archived: boolean): Result<null>;
}

/** The fields a channel summary gains with those commands. */
export type ChannelExtras = { name?: string | null; avatar?: MediaFile | null; archived?: boolean };

export function ext(api: Api): Commands & ExtCommands {
  return api.commands as Commands & ExtCommands;
}

export const channelName = (c: ChannelSummary): string | null => (c as ChannelExtras).name ?? null;
export const channelAvatar = (c: ChannelSummary): MediaFile | null => (c as ChannelExtras).avatar ?? null;
export const channelArchived = (c: ChannelSummary): boolean => (c as ChannelExtras).archived ?? false;
