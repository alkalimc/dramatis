import { useEffect, useMemo, type KeyboardEvent } from "react";
import type { ChannelSummary } from "../api";
import { useT } from "../i18n";
import { activityText, authorName, typingText, when } from "../lib/format";
import { byActivity, lastActivity, previewOf, unreadOf } from "../lib/chats";
import { useActions, useStore } from "../store";
import { Avatar } from "../ui/Avatar";

/** Arrow keys move between rows; each row is an ordinary button, so Tab works as well. */
function onListKey(e: KeyboardEvent<HTMLUListElement>) {
  if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
  const rows = [...e.currentTarget.querySelectorAll<HTMLButtonElement>("button.chat-row")];
  const at = rows.indexOf(document.activeElement as HTMLButtonElement);
  const next = rows[at + (e.key === "ArrowDown" ? 1 : -1)];
  if (next) {
    e.preventDefault();
    next.focus();
  }
}

/** The channels, most recent first: direct chats, groups and the host. */
export function ChatList() {
  const t = useT();
  const actions = useActions();
  const channels = useStore((s) => s.channels);
  const messages = useStore((s) => s.messages);
  const host = useStore((s) => s.hostChannel);
  const rows = useMemo(() => {
    const all = host && !channels.some((c) => c.id === host) ? [{ id: host, kind: "host", origin: "system", participants: [], topic: null, mode: "enabled", last_at: null } satisfies ChannelSummary, ...channels] : channels;
    return byActivity(all, messages);
  }, [channels, messages, host]);

  // Previews need the newest message of each channel; this loads it once per channel.
  // Keyed on the ids: loading changes `rows` (it re-sorts), and an empty log reloads.
  const ids = rows.map((c) => c.id).join("\n");
  useEffect(() => {
    for (const id of ids.split("\n")) if (id) void actions.loadHistory(id);
  }, [actions, ids]);

  return (
    <nav className="chat-list" aria-label={t("chatlist.label")}>
      {rows.length === 0 ? (
        <p className="muted pad">{t("chatlist.empty")}</p>
      ) : (
        <ul onKeyDown={onListKey}>
          {rows.map((c) => (
            <li key={c.id}>
              <ChatRow channel={c} />
            </li>
          ))}
        </ul>
      )}
    </nav>
  );
}

function ChatRow({ channel }: { channel: ChannelSummary }) {
  const t = useT();
  const actions = useActions();
  const names = useStore((s) => s.names);
  const user = useStore((s) => s.settings?.user_name ?? null);
  const marker = useStore((s) => s.wording.scene_marker);
  const list = useStore((s) => s.messages[channel.id]);
  const shown = useStore((s) => (s.channel ?? s.hostChannel) === channel.id);
  const unread = useStore((s) => unreadOf(channel.id, s.unread, s.digest));
  const waiting = useStore((s) => s.digest.items.find((w) => w.channel === channel.id));
  const who = useStore((s) => s.typing[channel.id]);

  const host = channel.kind === "host";
  const person = channel.kind === "direct" ? channel.participants[0] : undefined;
  const title = host ? t("host.name") : person ? person.name : (channel.topic ?? t("channel.group.untitled"));
  const last = list?.[list.length - 1];
  const at = lastActivity(channel, list);
  const typing = typingText(who, t, names, user);

  let preview = "";
  if (last) {
    const text = previewOf(last, marker);
    preview =
      last.author.kind === "user"
        ? t("chatlist.preview.you", { text })
        : channel.kind === "group"
          ? t("chatlist.preview.author", { name: authorName(last.author, t, names, user), text })
          : text;
  } else if (waiting) {
    preview = activityText(waiting.activity, t);
  }

  return (
    <button
      type="button"
      className={`chat-row${shown ? " selected" : ""}${unread > 0 ? " unread" : ""}`}
      aria-current={shown ? "true" : undefined}
      onClick={() => void actions.open(host ? null : channel.id)}
    >
      <Avatar person={person ?? { id: channel.id, name: title }} plain={!person} />
      <span className="chat-main">
        <span className="chat-top">
          <span className="chat-name">{title}</span>
          {at && <time className="chat-time" dateTime={at}>{when(at)}</time>}
        </span>
        <span className="chat-bottom">
          <span className={`chat-preview${typing ? " typing" : ""}`}>{typing ?? preview}</span>
          {unread > 0 && (
            <span className="badge" role="img" aria-label={t("chatlist.unread", { count: unread })}>
              {unread}
            </span>
          )}
        </span>
      </span>
    </button>
  );
}
