import { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { ChannelId, ChannelSummary, ImagePart, Message, PersonRef } from "../api";
import { useT, useUserName } from "../i18n";
import { channelTitle } from "../lib/chats";
import { authorName, dayKey, dayLabel, sameAuthor, typingText } from "../lib/format";
import { pastedImage, readImage } from "../lib/image";
import { isMac, isSendKey } from "../lib/keys";
import { filterMembers, insertMention, mentionAt } from "../lib/mention";
import { isBlank, speechOnly } from "../lib/scene";
import { useActions, useApi, useDispatch, useStore } from "../store";
import { quotaExhaustedLine } from "../lib/quota";
import { Avatar } from "../ui/Avatar";
import { Badge } from "../ui/Badge";
import { InlineError } from "../ui/ErrorBar";
import { Icon } from "../ui/Icon";
import { ActionLines, MessageView, StreamView, hueOf } from "./Message";
import { useChatMenu } from "./chatMenu";

const NO_MESSAGES: Message[] = [];

/** Ids of this channel's streams. Selecting the joined string keeps the body from re-rendering on every token. */
function useStreamIds(channel: ChannelId): string[] {
  const key = useStore((s) =>
    Object.entries(s.streams)
      .filter(([, st]) => st.channel === channel)
      .map(([id]) => id)
      .join("\n"),
  );
  return useMemo(() => (key ? key.split("\n") : []), [key]);
}

/** Messages by one speaker this close together read as one run of bubbles. */
const RUN_GAP_MS = 5 * 60_000;

function continues(a: Message, b: Message): boolean {
  return !a.service && !b.service && sameAuthor(a.author, b.author) && Math.abs(Date.parse(b.at) - Date.parse(a.at)) < RUN_GAP_MS;
}

/** One conversation: the header, the log, anything being written, and the composer. */
export function ChannelView({ channel, pinned, after }: { channel: ChannelId; pinned?: ReactNode; after?: (m: Message, i: number) => ReactNode }) {
  const t = useT();
  const actions = useActions();
  const dispatch = useDispatch();
  const messages = useStore((s) => s.messages[channel] ?? NO_MESSAGES);
  const done = useStore((s) => s.historyDone[channel] ?? false);
  const tts = useStore((s) => Boolean(s.endpoints?.roles?.tts));
  const streams = useStreamIds(channel);
  const log = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const historyError = useStore((s) => s.loadErrors[`history:${channel}`]);
  const typing = useStore((s) => Boolean(s.typing[channel]));
  const waiting = useStore((s) => Boolean(s.awaiting[channel]));
  const isHost = useStore((s) => s.hostChannel === channel);
  const favorites = useStore((s) => s.channels.find((c) => c.id === channel)?.kind === "favorites");
  const quota = useStore((s) => s.quota);
  const exhausted = isHost ? quotaExhaustedLine(quota, t) : null;
  const lastMessage = messages[messages.length - 1];
  const find = useStore((s) => s.find);
  const shown = messages;
  const marker = useStore((s) => s.wording.scene_marker);
  // Runs are read across messages that draw nothing: their neighbours are the ones on screen.
  const { prev, next } = useMemo(() => {
    const drawn = shown.map((m) => !isBlank(m, marker));
    const prev: number[] = [];
    const next: number[] = [];
    let last = -1;
    for (let i = 0; i < shown.length; i++) {
      prev[i] = last;
      if (drawn[i]) last = i;
    }
    last = -1;
    for (let i = shown.length - 1; i >= 0; i--) {
      next[i] = last;
      if (drawn[i]) last = i;
    }
    return { prev, next };
  }, [shown, marker]);

  useEffect(() => {
    void actions.loadHistory(channel);
  }, [actions, channel]);

  // Follow new messages only when the reader is already at the bottom.
  useLayoutEffect(() => {
    const el = log.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [messages, streams, waiting]);
  const follow = useCallback(() => {
    const el = log.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, []);

  // How far from the newest message the reader is, and how many arrived while away.
  const [away, setAway] = useState(0);
  const [unseen, setUnseen] = useState(0);
  const seen = useRef(messages.length);
  useEffect(() => {
    if (stick.current) setUnseen(0);
    else setUnseen((n) => n + Math.max(0, messages.length - seen.current));
    seen.current = messages.length;
  }, [messages.length]);

  const onScroll = () => {
    const el = log.current;
    if (!el) return;
    const gap = el.scrollHeight - el.scrollTop - el.clientHeight;
    stick.current = gap < 48;
    setAway(gap < 160 ? 0 : gap);
    if (gap < 48) setUnseen(0);
  };
  const toBottom = () => {
    const el = log.current;
    if (el) el.scrollTop = el.scrollHeight;
  };

  return (
    <section className="channel" aria-label={t("composer.label")}>
      <ChannelHeader channel={channel} finding={find !== null} onFind={() => dispatch({ type: "find", query: find === null ? "" : null })} />
      {pinned}
      <div className="log-wrap">
      <div ref={log} className="log" onScroll={onScroll} role="log" aria-live="polite" aria-relevant="additions" tabIndex={0}>
        {!done && messages.length > 0 && (
          <button type="button" className="link earlier" onClick={() => actions.loadHistory(channel, true)}>
            {t("channel.earlier")}
          </button>
        )}
        {messages.length === 0 && streams.length === 0 && <p className="muted empty">{t(favorites ? "favorites.empty" : "channel.empty")}</p>}
        {shown.map((m, i) => (
          <div key={m.id} className="log-item">
            {(i === 0 || dayKey(m.at) !== dayKey(shown[i - 1].at)) && (
              <p className="date-pill" role="separator">
                <time dateTime={m.at}>{dayLabel(m.at, t)}</time>
              </p>
            )}
            <MessageView
              message={m}
              showPlay={tts}
              first={!(prev[i] >= 0 && continues(shown[prev[i]], m))}
              last={!(next[i] >= 0 && continues(m, shown[next[i]]))}
            />
            {after?.(m, i)}
          </div>
        ))}
        {streams.map((id) => (
          <PendingActions key={id} id={id} channel={channel} last={lastMessage} onGrow={follow} />
        ))}
        {waiting && streams.length === 0 && !typing && (
          <p className="muted awaiting" role="status">
            {t("message.awaiting")}
          </p>
        )}
        <TurnFailed channel={channel} />
        {exhausted && <p className="host-line">{exhausted}</p>}
        {historyError && <InlineError error={historyError} onRetry={() => void actions.loadHistory(channel)} />}
      </div>
      {away > 0 && (
        <button type="button" className="to-bottom round" aria-label={t("channel.latest")} title={t("channel.latest")} onClick={toBottom}>
          {unseen > 0 && <Badge count={unseen} className="to-bottom-count" decorative />}
          <Icon name="down" size={24} />
        </button>
      )}
      </div>
      <Composer key={channel} channel={channel} />
    </section>
  );
}

/** Why the last turn here ended without a reply, until the next send or activity. */
function TurnFailed({ channel }: { channel: ChannelId }) {
  const t = useT();
  const failure = useStore((s) => s.failures[channel]);
  const names = useStore((s) => s.names);
  const user = useStore((s) => s.settings?.user_name ?? null);
  if (!failure) return null;
  return (
    <p className="turn-failed" role="alert">
      {t(`error.turn.${failure.reason}`, { name: authorName(failure.author, t, names, user) })}
    </p>
  );
}

function PendingActions({ id, channel, last, onGrow }: { id: string; channel: ChannelId; last?: Message; onGrow: () => void }) {
  const stream = useStore((s) => s.streams[id]);
  const pending = useStore((s) => s.pendingActions[id]);
  const text = stream?.text;
  useLayoutEffect(onGrow, [text, pending, onGrow]);
  if (!stream) return null;
  const first = !(last && sameAuthor(last.author, stream.author));
  return (
    <>
      {pending && <ActionLines author={stream.author} actions={pending} />}
      <StreamView author={stream.author} text={stream.text} channel={channel} first={first} />
    </>
  );
}

/** What the reply bar and a quote show of a message: its first spoken line. */
function snippetOf(m: Message, marker: { open: string; close: string }, picture: string): string {
  const line = speechOnly(m.text, marker)
    .split("\n")
    .map((l) => l.trim())
    .find(Boolean);
  return line ?? (m.attachment || m.picture_lost ? picture : "");
}

export function Composer({ channel }: { channel: ChannelId }) {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const dispatch = useDispatch();
  const id = useId();
  const saved = useStore((s) => s.drafts[channel] ?? "");
  const [text, setText] = useState(saved);
  const [image, setImage] = useState<ImagePart | null>(null);
  const [busy, setBusy] = useState(false);
  const [tray, setTray] = useState(false);
  const file = useRef<HTMLInputElement>(null);
  const box = useRef<HTMLTextAreaElement>(null);
  const restore = useStore((s) => (s.restore?.channel === channel ? s.restore : null));
  const kind = useStore((s) => s.channels.find((c) => c.id === channel)?.kind);
  const members = useStore((s) => (s.channels.find((c) => c.id === channel)?.kind === "group" ? s.channels.find((c) => c.id === channel)?.participants : undefined));
  const replyTo = useStore((s) => s.replies[channel]);
  const names = useStore((s) => s.names);
  const user = useStore((s) => s.settings?.user_name ?? null);
  const marker = useStore((s) => s.wording.scene_marker);
  const note = kind === "favorites";

  // `@` in a group opens the members; the caret says which word is being typed.
  const [caret, setCaret] = useState(0);
  const [pick, setPick] = useState(0);
  const [dismissed, setDismissed] = useState<number | null>(null);
  const typed = members ? mentionAt(text, caret) : null;
  const options = useMemo(() => (typed && members ? filterMembers(members, typed.query) : []), [typed?.query, members]);
  const listing = typed !== null && typed.start !== dismissed && options.length > 0;
  const chosen = Math.min(pick, Math.max(0, options.length - 1));
  const after = useRef<number | null>(null);

  // A message taken back returns here: ahead of whatever was already being typed.
  useEffect(() => {
    if (!restore) return;
    setText((cur) => (cur.trim() ? `${restore.text}\n${cur}` : restore.text));
    dispatch({ type: "restore", restore: null });
    box.current?.focus();
  }, [restore, dispatch]);

  // What is typed stays with the conversation: leaving and coming back finds it again.
  useEffect(() => {
    if (saved !== text) dispatch({ type: "draft", channel, text });
  }, [saved, channel, text, dispatch]);

  // Grow with the text up to the CSS max height, then scroll.
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  }, [text]);

  // A reply asked for from a message: write straight away.
  useEffect(() => {
    if (replyTo) box.current?.focus();
  }, [replyTo]);

  // The caret goes after an inserted `@name`.
  useLayoutEffect(() => {
    if (after.current === null || !box.current) return;
    box.current.setSelectionRange(after.current, after.current);
    setCaret(after.current);
    after.current = null;
  }, [text]);

  const take = (m: PersonRef) => {
    if (!typed) return;
    const next = insertMention(text, typed, caret, m.name);
    after.current = next.caret;
    setText(next.text);
    setPick(0);
    box.current?.focus();
  };

  const send = async () => {
    if (busy || (!text.trim() && !image)) return;
    setBusy(true);
    // Replies arrive as events; until the first one, say so. Set before the call so an
    // early event cannot be overtaken, and undone if the send itself fails. A note in
    // the favorites is answered by no one.
    if (!note) dispatch({ type: "awaiting", channel, on: true });
    const ok = await actions.attempt(api.commands.sendMessage(channel, text.trim(), image, replyTo?.id ?? null));
    setBusy(false);
    if (ok === undefined) return note ? undefined : dispatch({ type: "awaiting", channel, on: false });
    setText("");
    setImage(null);
    if (replyTo) dispatch({ type: "reply", channel, message: null });
  };

  const replyName = replyTo ? authorName(replyTo.author, t, names, user) : "";

  return (
    <form
      className="composer"
      onSubmit={(e) => {
        e.preventDefault();
        void send();
      }}
    >
      {replyTo && (
        <div className="reply-bar" role="group" aria-label={t("composer.reply", { name: replyName })}>
          <Icon name="reply" size={20} />
          <div className={`reply-quote hue-${hueOf(replyTo.author)}`}>
            <span className="quote-name">{replyName}</span>
            <span className="quote-text">{snippetOf(replyTo, marker, t("message.quote.picture"))}</span>
          </div>
          <button type="button" className="round small" aria-label={t("composer.reply.cancel")} title={t("composer.reply.cancel")} onClick={() => dispatch({ type: "reply", channel, message: null })}>
            <Icon name="close" size={16} />
          </button>
        </div>
      )}
      {image && (
        <div className="attached chip-attached">
          <img src={`data:${image.mime};base64,${image.data_base64}`} alt={t("composer.image.preview")} />
          <button type="button" className="round small" aria-label={t("composer.image.remove")} title={t("composer.image.remove")} onClick={() => setImage(null)}>
            <Icon name="close" size={16} />
          </button>
        </div>
      )}
      <div className="composer-row">
        <input
          ref={file}
          type="file"
          accept="image/png,image/jpeg,image/webp,image/gif"
          className="sr-only"
          tabIndex={-1}
          aria-hidden="true"
          onChange={async (e) => {
            const f = e.target.files?.[0];
            if (f) setImage(await readImage(f));
            e.target.value = "";
          }}
        />
        {/* The paperclip opens the picker at once; hovering or focusing it lists the kinds that work. */}
        <div
          className="attach"
          onMouseEnter={() => setTray(true)}
          onMouseLeave={() => setTray(false)}
          onFocus={() => setTray(true)}
          onBlur={(e) => {
            if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setTray(false);
          }}
          onKeyDown={(e) => {
            if (e.key === "Escape" && tray) {
              e.stopPropagation();
              setTray(false);
            }
          }}
        >
          <button
            type="button"
            className="round"
            aria-label={t("composer.attach")}
            title={t("composer.attach")}
            onClick={() => {
              setTray(false);
              file.current?.click();
            }}
          >
            <Icon name="attach" />
          </button>
          {tray && (
            <div className="attach-tray">
              <div className="menu-pop" role="menu" aria-label={t("composer.attach")}>
                <button
                  type="button"
                  role="menuitem"
                  onClick={() => {
                    setTray(false);
                    file.current?.click();
                  }}
                >
                  <Icon name="image" size={20} />
                  <span>{t("composer.image")}</span>
                </button>
              </div>
            </div>
          )}
        </div>
        <label htmlFor={id} className="sr-only">
          {t("composer.label")}
        </label>
        <div className="composer-field">
          {listing && (
            <ul className="mention-pop" role="listbox" aria-label={t("composer.mention")}>
              {options.map((m, i) => (
                <li key={m.id} id={`${id}-m${i}`} role="option" aria-selected={i === chosen} className={i === chosen ? "on" : undefined}>
                  <button type="button" tabIndex={-1} onMouseDown={(e) => e.preventDefault()} onClick={() => take(m)}>
                    <Avatar person={m} size="s" />
                    <span>{m.name}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
          <textarea
            ref={box}
            id={id}
            rows={1}
            aria-describedby={`${id}-hint`}
            aria-keyshortcuts={isMac() ? "Meta+Enter" : "Control+Enter"}
            aria-autocomplete={members ? "list" : undefined}
            aria-activedescendant={listing ? `${id}-m${chosen}` : undefined}
            value={text}
            placeholder={t(note ? "composer.note" : "composer.placeholder")}
            onChange={(e) => {
              setText(e.target.value);
              setCaret(e.target.selectionStart);
              setPick(0);
              setDismissed(null);
            }}
            onSelect={(e) => setCaret(e.currentTarget.selectionStart)}
            onPaste={(e) => {
              // A picture on the clipboard becomes the attachment; plain text pastes as usual.
              const f = pastedImage(e.clipboardData);
              if (!f) return;
              e.preventDefault();
              void readImage(f).then(setImage, () => undefined);
            }}
            onKeyDown={(e) => {
              if (isSendKey(e)) {
                e.preventDefault();
                void send();
                return;
              }
              if (e.nativeEvent.isComposing || e.keyCode === 229) return;
              if (listing) {
                if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                  e.preventDefault();
                  setPick((chosen + (e.key === "ArrowDown" ? 1 : options.length - 1)) % options.length);
                } else if ((e.key === "Enter" || e.key === "Tab") && !e.shiftKey) {
                  e.preventDefault();
                  take(options[chosen]);
                } else if (e.key === "Escape") {
                  e.preventDefault();
                  e.stopPropagation();
                  setDismissed(typed?.start ?? null);
                }
                return;
              }
              // Escape drops the reply being written.
              if (e.key === "Escape" && replyTo) {
                e.stopPropagation();
                dispatch({ type: "reply", channel, message: null });
              }
            }}
          />
        </div>
        <button type="submit" className="round send" aria-label={t("composer.send")} title={t("composer.send")} disabled={busy || (!text.trim() && !image)} aria-busy={busy}>
          <Icon name="send" />
        </button>
      </div>
      <p id={`${id}-hint`} className="sr-only">
        {t(isMac() ? "composer.hint.mac" : "composer.hint.other")}
      </p>
    </form>
  );
}

/** Title bar of a conversation: picture, name, who is typing or how many are here, and the tools. */
export function ChannelHeader({ channel, finding = false, onFind }: { channel: ChannelId; finding?: boolean; onFind?: () => void }) {
  const t = useT();
  const dispatch = useDispatch();
  const listed = useStore((s) => s.channels.find((c) => c.id === channel) ?? (s.temp?.id === channel ? s.temp : undefined));
  const isHost = useStore((s) => s.hostChannel === channel);
  const who = useStore((s) => s.typing[channel]);
  const names = useStore((s) => s.names);
  const user = useStore((s) => s.settings?.user_name ?? null);
  const drawer = useStore((s) => s.drawer);
  const menu = useChatMenu();
  const typing = typingText(who, t, names, user);
  const me = useUserName();

  const summary: ChannelSummary | undefined = listed ?? (isHost ? ({ id: channel, kind: "host", origin: "system", participants: [], topic: null, mode: "enabled", last_at: null, name: null, avatar: null, archived: false, unread: 0 } satisfies ChannelSummary) : undefined);
  if (!summary) return typing ? <header className="channel-header"><p className="subtitle typing" role="status">{typing}</p></header> : null;

  const person = summary.kind === "direct" ? summary.participants[0] : undefined;
  const info =
    summary.kind === "group" ? ({ kind: "channel", id: channel } as const) : summary.kind === "host" ? ({ kind: "host" } as const) : person ? ({ kind: "person", id: person.id } as const) : null;
  // The same icon opens the info panel and closes it again.
  const infoOpen = Boolean(info && drawer && drawer.kind === info.kind && (info.kind === "host" || (drawer.kind !== "host" && drawer.id === info.id)));
  // The user is a member of every group they are in.
  const fallback =
    summary.kind === "host"
      ? t("host.role")
      : summary.kind === "favorites"
        ? t("favorites.subtitle")
        : summary.kind === "group"
          ? t("group.members.count", { count: summary.participants.length + 1 })
          : t(`mode.${summary.mode}`);

  return (
    <header className="channel-header">
      <div className="titles">
        <h2 title={channelTitle(summary, t, me)}>{channelTitle(summary, t, me)}</h2>
        <p className={`subtitle${typing ? " typing" : ""}`} role="status">
          {typing ?? fallback}
        </p>
      </div>
      {onFind && (
        <button type="button" className="round" aria-label={t("channel.find")} title={t("channel.find")} aria-pressed={finding} onClick={onFind}>
          <Icon name="search" />
        </button>
      )}
      {info && (
        <button
          type="button"
          className="round"
          aria-label={t(summary.kind === "group" ? "channel.group_info" : "channel.person_page")}
          title={t(summary.kind === "group" ? "channel.group_info" : "channel.person_page")}
          aria-pressed={infoOpen}
          onClick={() => dispatch({ type: "drawer", drawer: infoOpen ? null : info })}
        >
          <Icon name="panel" />
        </button>
      )}
      {summary.kind !== "favorites" && (
        <button
          type="button"
          className="round"
          aria-label={t("channel.more")}
          title={t("channel.more")}
          aria-haspopup="menu"
          onClick={(e) => {
            const r = e.currentTarget.getBoundingClientRect();
            menu.show(summary, r.right - 200, r.bottom + 4);
          }}
        >
          <Icon name="more" />
        </button>
      )}
      {menu.menu}
    </header>
  );
}
