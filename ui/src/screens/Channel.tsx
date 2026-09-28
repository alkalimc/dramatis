import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { ChannelId, ImagePart, Message } from "../api";
import { useT } from "../i18n";
import { useActions, useApi, useDispatch, useStore, type Stream } from "../store";
import { ModeSwitch } from "../ui/ModeSwitch";
import { ActionLines, MessageView, StreamView } from "./Message";

const NO_MESSAGES: Message[] = [];

function useStreams(channel: ChannelId): [string, Stream][] {
  const streams = useStore((s) => s.streams);
  return useMemo(() => Object.entries(streams).filter(([, st]) => st.channel === channel), [streams, channel]);
}

/** One conversation: the log, anything being written, and the composer. */
export function ChannelView({ channel, header, after }: { channel: ChannelId; header?: ReactNode; after?: (m: Message, i: number) => ReactNode }) {
  const t = useT();
  const actions = useActions();
  const messages = useStore((s) => s.messages[channel] ?? NO_MESSAGES);
  const done = useStore((s) => s.historyDone[channel] ?? false);
  const tts = useStore((s) => Boolean(s.endpoints?.roles?.tts));
  const streams = useStreams(channel);
  const log = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  useEffect(() => {
    void actions.loadHistory(channel);
  }, [actions, channel]);

  // Follow new messages only when the reader is already at the bottom.
  useLayoutEffect(() => {
    const el = log.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [messages, streams]);

  const onScroll = () => {
    const el = log.current;
    if (el) stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
  };

  return (
    <section className="channel" aria-label={t("composer.label")}>
      {header}
      <div ref={log} className="log" onScroll={onScroll} role="log" aria-live="polite" aria-relevant="additions" tabIndex={0}>
        {!done && messages.length > 0 && (
          <button type="button" className="link earlier" onClick={() => actions.loadHistory(channel, true)}>
            {t("channel.earlier")}
          </button>
        )}
        {messages.length === 0 && streams.length === 0 && <p className="muted empty">{t("channel.empty")}</p>}
        {messages.map((m, i) => (
          <div key={m.id}>
            <MessageView message={m} showPlay={tts} />
            {after?.(m, i)}
          </div>
        ))}
        {streams.map(([id, st]) => (
          <PendingActions key={id} id={id} stream={st} />
        ))}
      </div>
      <Composer channel={channel} />
    </section>
  );
}

function PendingActions({ id, stream }: { id: string; stream: Stream }) {
  const pending = useStore((s) => s.pendingActions[id]);
  return (
    <>
      {pending && <ActionLines author={stream.author} actions={pending} />}
      <StreamView author={stream.author} text={stream.text} />
    </>
  );
}

function readImage(file: File): Promise<ImagePart> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onerror = () => reject(r.error);
    r.onload = () => {
      const url = String(r.result);
      resolve({ mime: file.type, data_base64: url.slice(url.indexOf(",") + 1) });
    };
    r.readAsDataURL(file);
  });
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

  const send = async () => {
    if (busy || (!text.trim() && !image)) return;
    setBusy(true);
    const ok = await actions.attempt(api.commands.sendMessage(channel, text.trim(), image));
    setBusy(false);
    if (ok !== undefined) {
      setText("");
      setImage(null);
    }
  };

  return (
    <form
      className="composer"
      onSubmit={(e) => {
        e.preventDefault();
        void send();
      }}
    >
      <label htmlFor={id} className="sr-only">
        {t("composer.label")}
      </label>
      <textarea
        id={id}
        rows={2}
        value={text}
        placeholder={t("composer.placeholder")}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
            e.preventDefault();
            void send();
          }
        }}
      />
      {image && (
        <p className="attached">
          <img src={`data:${image.mime};base64,${image.data_base64}`} alt="" />
          <button type="button" className="link" onClick={() => setImage(null)}>
            {t("composer.image.remove")}
          </button>
        </p>
      )}
      <div className="composer-actions">
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
        <button type="button" className="ghost" onClick={() => file.current?.click()}>
          {t("composer.image")}
        </button>
        <span className="spacer" />
        <button type="button" className="ghost" onClick={() => dispatch({ type: "ask", draft: {} })}>
          ⊕ {t("composer.ask")}
        </button>
        <button type="button" className="ghost" onClick={() => dispatch({ type: "groupDialog", open: true })}>
          ⊕ {t("composer.group")}
        </button>
        <button type="submit" disabled={busy || (!text.trim() && !image)}>
          {t("composer.send")}
        </button>
      </div>
    </form>
  );
}

/** Title bar of a direct or group channel. */
export function ChannelHeader({ channel }: { channel: ChannelId }) {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const dispatch = useDispatch();
  const summary = useStore((s) => s.channels.find((c) => c.id === channel));
  const [menu, setMenu] = useState(false);
  if (!summary) return null;

  if (summary.kind === "direct") {
    const p = summary.participants[0];
    return (
      <header className="channel-header">
        <h2>{p?.name ?? t("person.unknown")}</h2>
        {p && (
          <button type="button" className="link" onClick={() => dispatch({ type: "drawer", drawer: { kind: "person", id: p.id } })}>
            {t("channel.person_page")}
          </button>
        )}
      </header>
    );
  }

  const names = summary.participants.map((p) => p.name).join(" · ");
  const remove = async () => {
    if (!window.confirm(t("channel.delete.confirm"))) return;
    const ok = await actions.attempt(api.commands.deleteGroup(channel));
    if (ok !== undefined) {
      await actions.refreshChannels();
      await actions.open(null);
    }
  };

  return (
    <header className="channel-header group">
      <div>
        <h2>{summary.topic ?? t("channel.group.untitled")}</h2>
        <p className="muted small">{t("channel.group.members", { names })}</p>
      </div>
      <ModeSwitch
        kind="group"
        mode={summary.mode}
        label={t("mode.label.group")}
        onChange={(m) => actions.setMode({ kind: "channel", id: channel }, m)}
      />
      <div className="menu">
        <button type="button" className="icon" aria-label={t("channel.more")} aria-expanded={menu} onClick={() => setMenu((v) => !v)}>
          ⋯
        </button>
        {menu && (
          <div className="menu-items">
            <button type="button" className="danger" onClick={remove}>
              {t("channel.delete")}
            </button>
          </div>
        )}
      </div>
    </header>
  );
}
