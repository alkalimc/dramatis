import { useEffect, useState } from "react";
import type { Author, ChannelSummary, PersonRef } from "../api";
import { useT, useUserName } from "../i18n";
import { channelTitle } from "../lib/chats";
import { useApi, useStore } from "../store";
import { Icon } from "./Icon";

type Size = "s" | "m" | "l" | "xl";

// Fetched on demand, once per person per session; `null` = none, fall back to a letter.
const cache = new Map<string, Promise<string | null>>();

const initialOf = (name: string) => Array.from(name.trim())[0] ?? "?";

/** The host's id wherever it stands in for a person (an author, a participant): its picture is the corpus's host portrait. */
export const HOST_ID = "host";

/** `plain` skips the portrait lookup: the initial only. */
export function Avatar({ person, size = "m", plain = false }: { person: PersonRef; size?: Size; plain?: boolean }) {
  const api = useApi();
  const [src, setSrc] = useState<string | null>(null);

  useEffect(() => {
    if (plain) return;
    let live = true;
    let p = cache.get(person.id);
    if (!p) {
      p = (person.id === HOST_ID ? api.commands.hostAvatar() : api.commands.avatar(person.id)).then((r) => (r.status === "ok" && r.data ? api.mediaUrl(r.data.path) : null));
      cache.set(person.id, p);
    }
    p.then((url) => live && setSrc(url)).catch(() => undefined);
    return () => {
      live = false;
    };
  }, [api, person.id, plain]);

  return (
    <span className={`avatar avatar-${size}`} aria-hidden="true">
      {src ? <img src={src} alt="" onError={() => setSrc(null)} /> : initialOf(person.name)}
    </span>
  );
}

/** The user's own picture: uploaded, else the corpus's default; the initial only when the engine has neither. */
export function UserAvatar({ size = "m" }: { size?: Size }) {
  const api = useApi();
  const t = useT();
  const file = useStore((s) => s.userAvatar);
  const name = useStore((s) => s.settings?.user_name ?? t("user.you"));
  const [broken, setBroken] = useState<string | null>(null);
  const src = file && broken !== file.path ? api.mediaUrl(file.path) : null;
  return (
    <span className={`avatar avatar-${size}`} aria-hidden="true">
      {src ? <img src={src} alt="" onError={() => setBroken(file?.path ?? null)} /> : initialOf(name)}
    </span>
  );
}

/**
 * A group's picture: the one the user set, else a grid of up to four faces. The user's own
 * takes the first place, the members in the order they joined fill the rest.
 */
function GroupAvatar({ channel, size }: { channel: ChannelSummary; size: Size }) {
  const api = useApi();
  const t = useT();
  const user = useUserName();
  const custom = channel.avatar;
  const members = channel.participants.slice(0, 3);
  if (custom) {
    return (
      <span className={`avatar avatar-${size}`} aria-hidden="true">
        <img src={api.mediaUrl(custom.path)} alt="" />
      </span>
    );
  }
  if (members.length === 0) return <Avatar person={{ id: channel.id, name: channelTitle(channel, t, user) }} size={size} plain />;
  return (
    <span className={`avatar avatar-${size} avatar-grid grid-${members.length + 1}`} aria-hidden="true">
      <UserAvatar size="s" />
      {members.map((p) => (
        <Avatar key={p.id} person={p} size="s" />
      ))}
    </span>
  );
}

/** Whatever picture stands for a conversation in lists and headers. */
export function ChannelAvatar({ channel, size = "m" }: { channel: ChannelSummary; size?: Size }) {
  const t = useT();
  if (channel.kind === "host") return <Avatar person={{ id: HOST_ID, name: t("host.name") }} size={size} />;
  // The favorites have a bookmark where others have a face.
  if (channel.kind === "favorites") {
    return (
      <span className={`avatar avatar-${size} avatar-favorites`} aria-hidden="true">
        <Icon name="bookmark" size={size === "s" ? 14 : size === "m" ? 20 : 26} />
      </span>
    );
  }
  if (channel.kind === "direct" && channel.participants[0]) return <Avatar person={channel.participants[0]} size={size} />;
  return <GroupAvatar channel={channel} size={size} />;
}

/** The picture of whoever wrote a message: the user's own, the host's portrait, or the person's. */
export function AuthorAvatar({ author, name, size = "m" }: { author: Author; name: string; size?: Size }) {
  if (author.kind === "user") return <UserAvatar size={size} />;
  return <Avatar person={{ id: author.kind === "host" ? HOST_ID : author.id, name }} size={size} />;
}
