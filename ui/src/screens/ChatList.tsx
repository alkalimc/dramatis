import { useEffect, useId, useMemo, useState, type KeyboardEvent } from "react";
import type { ChannelSummary } from "../api";
import { useT } from "../i18n";
import { activityText, authorName, typingText, when } from "../lib/format";
import { byActivity, channelTitle, lastActivity, previewOf, unreadOf } from "../lib/chats";
import { useActions, useApi, useDispatch, useStore } from "../store";
import { ChannelAvatar } from "../ui/Avatar";
import { Icon } from "../ui/Icon";
import { useChatMenu } from "./chatMenu";

/** Arrow keys move between rows; each row is an ordinary button, so Tab works as well. */
function onListKey(e: KeyboardEvent<HTMLElement>) {
  if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
  const rows = [...e.currentTarget.querySelectorAll<HTMLButtonElement>("button.chat-row")];
  const at = rows.indexOf(document.activeElement as HTMLButtonElement);
  const next = rows[at + (e.key === "ArrowDown" ? 1 : -1)];
  if (next) {
    e.preventDefault();
    next.focus();
  }
}

/** The in-world date and time on one quiet line, with the demo tag when the data is canned. */
function ClockLine() {
  const t = useT();
  const { mock } = useApi();
  const actions = useActions();
  const clock = useStore((s) => s.clock);

  // The clock is pure data; refresh it on the minute.
  useEffect(() => {
    const timer = setInterval(() => void actions.refreshClock(), 30_000);
    return () => clearInterval(timer);
  }, [actions]);

  const pad = (n: number) => String(n).padStart(2, "0");
  return (
    <footer className="list-foot">
      {clock && (
        <time className="clock" aria-label={t("clock.label")}>
          {t("clock.in_world", { year: clock.in_world.year, month: pad(clock.in_world.month), day: pad(clock.in_world.day), time: clock.local_time })}
        </time>
      )}
      <span className="spacer" />
      {mock && <span className="tag">{t("app.mock")}</span>}
    </footer>
  );
}

/** The channels, most recent first: pinned on top, then direct chats, groups and the host. */
export function ChatList() {
  const t = useT();
  const actions = useActions();
  const dispatch = useDispatch();
  const searchId = useId();
  const channels = useStore((s) => s.channels);
  const messages = useStore((s) => s.messages);
  const host = useStore((s) => s.hostChannel);
  const temp = useStore((s) => s.temp);
  const open = useStore((s) => s.channel);
  const pinned = useStore((s) => s.pinned);
  const view = useStore((s) => s.listView);
  const [query, setQuery] = useState("");

  const all = useMemo(() => {
    const list: ChannelSummary[] = host && !channels.some((c) => c.id === host) ? [{ id: host, kind: "host", origin: "system", participants: [], topic: null, mode: "enabled", last_at: null, name: null, avatar: null, archived: false } satisfies ChannelSummary, ...channels] : channels;
    // A conversation opened before anyone spoke has no row yet; it gets one while it is open.
    return temp && temp.id === open && !list.some((c) => c.id === temp.id) ? [...list, temp] : list;
  }, [channels, host, temp, open]);

  const archivedCount = all.filter((c) => c.archived).length;
  const q = query.trim().toLowerCase();
  const rows = useMemo(() => {
    // Searching looks everywhere, archived chats included; otherwise one list at a time.
    const scope = q ? all.filter((c) => channelTitle(c, t).toLowerCase().includes(q)) : all.filter((c) => c.archived === (view === "archived"));
    const sorted = byActivity(scope, messages);
    return [...sorted.filter((c) => pinned.includes(c.id)), ...sorted.filter((c) => !pinned.includes(c.id))];
  }, [all, messages, pinned, view, q, t]);

  // Previews need the newest message of each channel; this loads it once per channel.
  // Keyed on the ids: loading changes `rows` (it re-sorts), and an empty log reloads.
  const ids = all.map((c) => c.id).join("\n");
  useEffect(() => {
    for (const id of ids.split("\n")) if (id) void actions.loadHistory(id);
  }, [actions, ids]);

  const menu = useChatMenu();
  const archivedView = view === "archived" && !q;

  return (
    <nav className="chat-list" aria-label={t("chatlist.label")} onKeyDown={onListKey}>
      <header className="list-head">
        <button type="button" className="round" aria-label={t("menu.open")} title={t("menu.open")} onClick={() => dispatch({ type: "menu", open: true })}>
          <Icon name="menu" />
        </button>
        <div className="search-field">
          <Icon name="search" size={18} />
          <label htmlFor={searchId} className="sr-only">
            {t("chatlist.search")}
          </label>
          <input
            id={searchId}
            type="search"
            value={query}
            placeholder={t("chatlist.search")}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") setQuery("");
            }}
          />
        </div>
      </header>
      {archivedView && (
        <div className="list-sub">
          <button type="button" className="round" aria-label={t("chatlist.back")} title={t("chatlist.back")} onClick={() => dispatch({ type: "listView", view: "chats" })}>
            <Icon name="back" />
          </button>
          <h2>{t("chatlist.archived")}</h2>
        </div>
      )}
      <div className="list-scroll">
        {view === "chats" && !q && archivedCount > 0 && (
          <button type="button" className="chat-row archived-row" onClick={() => dispatch({ type: "listView", view: "archived" })}>
            <span className="avatar avatar-l archive-badge" aria-hidden="true">
              <Icon name="archive" size={24} />
            </span>
            <span className="chat-main">
              <span className="chat-top">
                <span className="chat-name">{t("chatlist.archived")}</span>
                <span className="chat-time">{archivedCount}</span>
              </span>
            </span>
          </button>
        )}
        {rows.length === 0 ? (
          <p className="muted pad">{q ? t("chatlist.search.empty") : archivedView ? t("chatlist.archived.empty") : t("chatlist.empty")}</p>
        ) : (
          <ul>
            {rows.map((c) => (
              <li key={c.id}>
                <ChatRow channel={c} onMenu={menu.show} pinned={pinned.includes(c.id)} />
              </li>
            ))}
          </ul>
        )}
      </div>
      <ClockLine />
      {menu.menu}
    </nav>
  );
}

function ChatRow({ channel, onMenu, pinned }: { channel: ChannelSummary; onMenu: (c: ChannelSummary, x: number, y: number) => void; pinned: boolean }) {
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
  const title = channelTitle(channel, t);
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
    <div className="chat-item">
      <button
        type="button"
        className={`chat-row${shown ? " selected" : ""}${unread > 0 ? " unread" : ""}`}
        aria-current={shown ? "true" : undefined}
        onClick={() => void actions.open(host ? null : channel.id)}
        onContextMenu={(e) => {
          e.preventDefault();
          onMenu(channel, e.clientX, e.clientY);
        }}
      >
        <ChannelAvatar channel={channel} size="l" />
        <span className="chat-main">
          <span className="chat-top">
            <span className="chat-name">{title}</span>
            {pinned && (
              <span className="chat-pin" role="img" aria-label={t("chatlist.pinned")}>
                <Icon name="pin" size={14} />
              </span>
            )}
            {at && <time className="chat-time" dateTime={at}>{when(at)}</time>}
          </span>
          <span className="chat-bottom">
            <span className={`chat-preview${typing ? " typing" : ""}`}>{typing ?? preview}</span>
          </span>
        </span>
      </button>
      {unread > 0 && (
        // A sibling of the row, not inside it: a button cannot hold a button.
        <button type="button" className="badge" aria-label={t("chatlist.unread", { count: unread })} title={t("chatlist.mark_read")} onClick={() => void actions.clearUnread(channel.id)}>
          {unread}
        </button>
      )}
    </div>
  );
}
