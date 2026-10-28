import { useEffect, useState } from "react";
import type { ChannelSummary, PersonRef } from "../api";
import { useT } from "../i18n";
import { channelTitle } from "../lib/chats";
import { useApi, useStore } from "../store";

type Size = "s" | "m" | "l" | "xl";

// Fetched on demand, once per person per session; `null` = none, fall back to a letter.
const cache = new Map<string, Promise<string | null>>();

const initialOf = (name: string) => Array.from(name.trim())[0] ?? "?";

/** `plain` skips the portrait lookup: the initial only (the host). */
export function Avatar({ person, size = "m", plain = false }: { person: PersonRef; size?: Size; plain?: boolean }) {
  const api = useApi();
  const [src, setSrc] = useState<string | null>(null);

  useEffect(() => {
    if (plain) return;
    let live = true;
    let p = cache.get(person.id);
    if (!p) {
      p = api.commands.avatar(person.id).then((r) => (r.status === "ok" && r.data ? api.mediaUrl(r.data.path) : null));
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

/** A group's picture: the one the user set, else its first four members in a 2x2 grid. */
function GroupAvatar({ channel, size }: { channel: ChannelSummary; size: Size }) {
  const api = useApi();
  const t = useT();
  const custom = channel.avatar;
  const members = channel.participants.slice(0, 4);
  if (custom) {
    return (
      <span className={`avatar avatar-${size}`} aria-hidden="true">
        <img src={api.mediaUrl(custom.path)} alt="" />
      </span>
    );
  }
  if (members.length < 2) {
    const name = members[0]?.name ?? channelTitle(channel, t);
    return members[0] ? <Avatar person={members[0]} size={size} /> : <Avatar person={{ id: channel.id, name }} size={size} plain />;
  }
  return (
    <span className={`avatar avatar-${size} avatar-grid grid-${members.length}`} aria-hidden="true">
      {members.map((p) => (
        <Avatar key={p.id} person={p} size="s" />
      ))}
    </span>
  );
}

/** Whatever picture stands for a conversation in lists and headers. */
export function ChannelAvatar({ channel, size = "m" }: { channel: ChannelSummary; size?: Size }) {
  const t = useT();
  if (channel.kind === "host") return <Avatar person={{ id: "host", name: t("host.name") }} size={size} plain />;
  if (channel.kind === "direct" && channel.participants[0]) return <Avatar person={channel.participants[0]} size={size} />;
  return <GroupAvatar channel={channel} size={size} />;
}
