import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { ChannelId, ChannelSummary, ImagePart, Message } from "../api";
import { useT } from "../i18n";
import { channelTitle } from "../lib/chats";
import { authorName, sameAuthor, typingText } from "../lib/format";
import { pastedImage, readImage } from "../lib/image";
import { isMac, isSendKey } from "../lib/keys";
import { useActions, useApi, useDispatch, useStore, type Stream } from "../store";
import { quotaExhaustedLine } from "../lib/quota";
import { ChannelAvatar } from "../ui/Avatar";
import { InlineError } from "../ui/ErrorBar";
import { Icon } from "../ui/Icon";
import { ActionLines, MessageView, StreamView } from "./Message";
import { useChatMenu } from "./chatMenu";

const NO_MESSAGES: Message[] = [];

function useStreams(channel: ChannelId): [string, Stream][] {
  const streams = useStore((s) => s.streams);
  return useMemo(() => Object.entries(streams).filter(([, st]) => st.channel === channel), [streams, channel]);
}

/** Messages by one speaker this close together read as one run of bubbles. */
const RUN_GAP_MS = 5 * 60_000;

function continues(a: Message, b: Message): boolean {
  return sameAuthor(a.author, b.author) && Math.abs(Date.parse(b.at) - Date.parse(a.at)) < RUN_GAP_MS;
}

/** One conversation: the header, the log, anything being written, and the composer. */
export function ChannelView({ channel, pinned, after }: { channel: ChannelId; pinned?: ReactNode; after?: (m: Message, i: number) => ReactNode }) {
  const t = useT();
  const actions = useActions();
  const messages = useStore((s) => s.messages[channel] ?? NO_MESSAGES);
  const done = useStore((s) => s.historyDone[channel] ?? false);
  const tts = useStore((s) => Boolean(s.endpoints?.roles?.tts));
  const streams = useStreams(channel);
  const log = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const historyError = useStore((s) => s.loadErrors[`history:${channel}`]);
  const typing = useStore((s) => Boolean(s.typing[channel]));
  const waiting = useStore((s) => Boolean(s.awaiting[channel]));
  const isHost = useStore((s) => s.hostChannel === channel);
  const quota = useStore((s) => s.quota);
  const exhausted = isHost ? quotaExhaustedLine(quota, t) : null;
  const lastMessage = messages[messages.length - 1];
  const [find, setFind] = useState<string | null>(null);
  const needle = find?.trim().toLowerCase() ?? "";
  const shown = useMemo(() => (needle ? messages.filter((m) => m.text.toLowerCase().includes(needle)) : messages), [messages, needle]);

  useEffect(() => {
    void actions.loadHistory(channel);
  }, [actions, channel]);

  // Follow new messages only when the reader is already at the bottom.
  useLayoutEffect(() => {
    const el = log.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [messages, streams, waiting]);

  const onScroll = () => {
    const el = log.current;
    if (el) stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
  };

  return (
    <section className="channel" aria-label={t("composer.label")}>
      <ChannelHeader channel={channel} finding={find !== null} onFind={() => setFind((f) => (f === null ? "" : null))} />
      {find !== null && <FindBar value={find} count={shown.length} onChange={setFind} onClose={() => setFind(null)} />}
      {pinned}
      <div ref={log} className="log" onScroll={onScroll} role="log" aria-live="polite" aria-relevant="additions" tabIndex={0}>
        {!done && messages.length > 0 && (
          <button type="button" className="link earlier" onClick={() => actions.loadHistory(channel, true)}>
            {t("channel.earlier")}
          </button>
        )}
        {messages.length === 0 && streams.length === 0 && <p className="muted empty">{t("channel.empty")}</p>}
        {needle && shown.length === 0 && <p className="muted empty">{t("channel.find.none")}</p>}
        {shown.map((m, i) => (
          <div key={m.id} className="log-item">
            <MessageView
              message={m}
              showPlay={tts}
              first={!(i > 0 && continues(shown[i - 1], m))}
              last={!(i < shown.length - 1 && continues(m, shown[i + 1]))}
            />
            {after?.(m, i)}
          </div>
        ))}
        {streams.map(([id, st]) => (
          <PendingActions key={id} id={id} stream={st} channel={channel} first={!(lastMessage && sameAuthor(lastMessage.author, st.author))} />
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
      <Composer channel={channel} />
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

function PendingActions({ id, stream, channel, first }: { id: string; stream: Stream; channel: ChannelId; first: boolean }) {
  const pending = useStore((s) => s.pendingActions[id]);
  return (
    <>
      {pending && <ActionLines author={stream.author} actions={pending} />}
      <StreamView author={stream.author} text={stream.text} channel={channel} first={first} />
    </>
  );
}

export function Composer({ channel }: { channel: ChannelId }) {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const dispatch = useDispatch();
  const id = useId();
  const [text, setText] = useState("");
  const [image, setImage] = useState<ImagePart | null>(null);
  const [busy, setBusy] = useState(false);
  const file = useRef<HTMLInputElement>(null);
  const box = useRef<HTMLTextAreaElement>(null);
  const restore = useStore((s) => (s.restore?.channel === channel ? s.restore : null));

  // A message taken back returns here: ahead of whatever was already being typed.
  useEffect(() => {
    if (!restore) return;
    setText((cur) => (cur.trim() ? `${restore.text}\n${cur}` : restore.text));
    dispatch({ type: "restore", restore: null });
    box.current?.focus();
  }, [restore, dispatch]);

  // Grow with the text up to the CSS max height, then scroll.
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  }, [text]);

  const send = async () => {
    if (busy || (!text.trim() && !image)) return;
    setBusy(true);
    // Replies arrive as events; until the first one, say so. Set before the call so an
    // early event cannot be overtaken, and undone if the send itself fails.
    dispatch({ type: "awaiting", channel, on: true });
    const ok = await actions.attempt(api.commands.sendMessage(channel, text.trim(), image));
    setBusy(false);
    if (ok === undefined) return dispatch({ type: "awaiting", channel, on: false });
    setText("");
    setImage(null);
  };

  return (
    <form
      className="composer"
      onSubmit={(e) => {
        e.preventDefault();
        void send();
      }}
    >
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
        <button type="button" className="round" aria-label={t("composer.image")} title={t("composer.image")} onClick={() => file.current?.click()}>
          <Icon name="attach" />
        </button>
        <label htmlFor={id} className="sr-only">
          {t("composer.label")}
        </label>
        <textarea
          ref={box}
          id={id}
          rows={1}
          aria-describedby={`${id}-hint`}
          aria-keyshortcuts={isMac() ? "Meta+Enter" : "Control+Enter"}
          value={text}
          placeholder={t("composer.placeholder")}
          onChange={(e) => setText(e.target.value)}
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
            }
          }}
        />
        <button type="submit" className="round send" aria-label={t("composer.send")} title={t("composer.send")} disabled={busy || (!text.trim() && !image)} aria-busy={busy}>
          <Icon name="send" />
        </button>
      </div>
      <div className="composer-meta">
        <p id={`${id}-hint`} className="muted small composer-hint">
          {t(isMac() ? "composer.hint.mac" : "composer.hint.other")}
        </p>
        <span className="spacer" />
        <button type="button" className="link" onClick={() => dispatch({ type: "ask", draft: {} })}>
          ⊕ {t("composer.ask")}
        </button>
        <button type="button" className="link" onClick={() => dispatch({ type: "groupDialog", open: true })}>
          ⊕ {t("composer.group")}
        </button>
      </div>
    </form>
  );
}

function FindBar({ value, count, onChange, onClose }: { value: string; count: number; onChange: (v: string) => void; onClose: () => void }) {
  const t = useT();
  const id = useId();
  return (
    <div className="find-bar" role="search">
      <Icon name="search" size={18} />
      <label htmlFor={id} className="sr-only">
        {t("channel.find")}
      </label>
      <input
        id={id}
        type="search"
        autoFocus
        value={value}
        placeholder={t("channel.find")}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => e.key === "Escape" && onClose()}
      />
      {value.trim() && <span className="muted small">{t("channel.find.count", { count })}</span>}
      <button type="button" className="round small" aria-label={t("drawer.close")} title={t("drawer.close")} onClick={onClose}>
        <Icon name="close" size={18} />
      </button>
    </div>
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

  const summary: ChannelSummary | undefined = listed ?? (isHost ? ({ id: channel, kind: "host", origin: "system", participants: [], topic: null, mode: "enabled", last_at: null, name: null, avatar: null, archived: false } satisfies ChannelSummary) : undefined);
  if (!summary) return typing ? <header className="channel-header"><p className="subtitle typing" role="status">{typing}</p></header> : null;

  const person = summary.kind === "direct" ? summary.participants[0] : undefined;
  const info = summary.kind === "group" ? ({ kind: "channel", id: channel } as const) : person ? ({ kind: "person", id: person.id } as const) : null;
  // The same icon opens the info panel and closes it again.
  const infoOpen = Boolean(info && drawer && drawer.kind === info.kind && drawer.id === info.id);
  const fallback = summary.kind === "host" ? t("host.role") : summary.kind === "group" ? t("group.members.count", { count: summary.participants.length }) : t(`mode.${summary.mode}`);

  return (
    <header className="channel-header">
      <ChannelAvatar channel={summary} />
      <div className="titles">
        <h2>{channelTitle(summary, t)}</h2>
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
      {menu.menu}
    </header>
  );
}
