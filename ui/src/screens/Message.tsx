import { memo, useCallback, useMemo, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import type { Author, Message, ToolAction } from "../api";
import { useT } from "../i18n";
import { actionText, authorName, choicesOf, nameList, when } from "../lib/format";
import { renderMarkdown } from "../lib/markdown";
import { bubbles, speechOnly } from "../lib/scene";
import { useActions, useApi, useDispatch, useStore } from "../store";
import { SourcesPanel } from "./Citation";
import { Avatar, UserAvatar } from "../ui/Avatar";
import { Dialog } from "../ui/Dialog";
import { Icon } from "../ui/Icon";
import { Menu, type MenuItem } from "../ui/Menu";
import { SelectionMenu } from "./SelectionMenu";

export function ActionLines({ author, actions }: { author: Author; actions: ToolAction[] }) {
  const t = useT();
  const names = useStore((s) => s.names);
  const user = useStore((s) => s.settings?.user_name ?? null);
  const shown = actions.filter((a) => a.tool !== "wrapup");
  if (shown.length === 0) return null;
  const name = authorName(author, t, names, user);
  return (
    <ul className="action-lines">
      {shown.map((a, i) => (
        <li key={i}>{t("action.line", { name, action: actionText(a, t, names) })}</li>
      ))}
    </ul>
  );
}

/** What the host or a person offered the user: each button takes them to a conversation, or makes the suggested group. */
function Choices({ actions, message }: { actions: ToolAction[]; message: Message }) {
  const t = useT();
  const acts = useActions();
  const api = useApi();
  const dispatch = useDispatch();
  const choices = useMemo(() => choicesOf(actions), [actions]);
  if (choices.length === 0) return null;
  return (
    <div className="choices message-offers">
      {choices.map((c) =>
        c.kind === "person" ? (
          <button key={`p:${c.id}`} type="button" className="ghost" onClick={() => acts.openDirect(c.id)}>
            {t("choice.person", { name: c.name })}
          </button>
        ) : c.kind === "new_group" ? (
          <button
            key={`n:${c.members.join(",")}`}
            type="button"
            className="ghost"
            onClick={async () => {
              // One group per suggestion: the engine records it on the line, so a second
              // click gets the same group.
              const group = await acts.attempt(api.commands.openOffer(message.id, c.action));
              if (!group) return;
              dispatch({ type: "offerTaken", channel: message.channel, message: message.id, action: c.action, group });
              await acts.openChannel(group);
            }}
          >
            {t("choice.new_group", { names: nameList(c.names, t) })}
          </button>
        ) : (
          <button key={`g:${c.channel}`} type="button" className="ghost" onClick={() => acts.openChannel(c.channel)}>
            {t("choice.group", { names: nameList(c.names, t) })}
          </button>
        ),
      )}
    </div>
  );
}

function Speech({ text }: { text: string }) {
  const html = useMemo(() => renderMarkdown(text), [text]);
  return <div className="speech" dangerouslySetInnerHTML={{ __html: html }} />;
}

/** A stable accent per speaker, so a name keeps its colour across the conversation. */
const HUES = 8;
function hueOf(a: Author): number {
  const key = a.kind === "person" ? a.id : a.kind;
  let h = 0;
  for (const ch of key) h = (h * 31 + ch.charCodeAt(0)) % 997;
  return h % HUES;
}

/** The speaker's round avatar, shown beside the last bubble of a run. */
function SpeakerAvatar({ author, name }: { author: Author; name: string }) {
  if (author.kind === "user") return null;
  return <Avatar person={{ id: author.kind === "person" ? author.id : "host", name }} size="m" plain={author.kind !== "person"} />;
}

function SceneLines({ lines }: { lines: string[] }) {
  return (
    <>
      {lines.map((l, i) => (
        <p key={i} className="scene">
          {l}
        </p>
      ))}
    </>
  );
}

/** The picture the user sent, or a placeholder when it is gone; a click opens it larger. */
function Picture({ message }: { message: Message }) {
  const t = useT();
  const api = useApi();
  const [open, setOpen] = useState(false);
  if (message.picture_lost) {
    return (
      <p className="attachment-lost">
        <Icon name="image" size={18} /> {t("message.picture.lost")}
      </p>
    );
  }
  if (!message.attachment) return null;
  const src = api.mediaUrl(message.attachment.path);
  return (
    <>
      <button type="button" className="attachment-open" aria-label={t("message.picture.open")} onClick={() => setOpen(true)}>
        <img className="attachment" src={src} alt={t("message.picture")} />
      </button>
      {open &&
        createPortal(
          <Dialog title={t("message.picture")} onClose={() => setOpen(false)} className="lightbox" hideTitle>
            <button type="button" className="lightbox-close" aria-label={t("drawer.close")} onClick={() => setOpen(false)}>
              <img src={src} alt={t("message.picture")} />
            </button>
          </Dialog>,
          document.body,
        )}
    </>
  );
}

function BubbleView({ b, label, foot, top }: { b: ReturnType<typeof bubbles>[number]; label: ReactNode; foot: ReactNode; top?: ReactNode }) {
  return (
    <div className="bubble">
      {label}
      {top}
      <SceneLines lines={b.before} />
      {b.text !== null && <Speech text={b.text} />}
      <SceneLines lines={b.after} />
      {foot}
    </div>
  );
}

/**
 * One message: a run of bubbles (a paragraph each) with the speaker's avatar beside the
 * last one of the run. `first` and `last` say where this message sits in a run by the same
 * speaker; only the first carries the name, only the last the avatar.
 */
export const MessageView = memo(function MessageView({
  message,
  showPlay,
  first = true,
  last = true,
}: {
  message: Message;
  showPlay: boolean;
  first?: boolean;
  last?: boolean;
}) {
  const t = useT();
  const api = useApi();
  const { attempt, resend } = useActions();
  const names = useStore((s) => s.names);
  const user = useStore((s) => s.settings?.user_name ?? null);
  const marker = useStore((s) => s.wording.scene_marker);
  // A direct chat has one other speaker; the name only helps where several can talk.
  const direct = useStore((s) => s.channels.find((c) => c.id === message.channel)?.kind === "direct");
  const parts = useMemo(() => bubbles(message.text, marker), [message.text, marker]);
  const sourcesOpen = useStore((s) => Boolean(s.sourcesOpen[message.id]));
  const dispatch = useDispatch();
  const after = useStore((s) => {
    const list = s.messages[message.channel];
    const i = list ? list.findIndex((m) => m.id === message.id) : -1;
    return i >= 0 && list ? list.length - i - 1 : 0;
  });
  // A reply being written holds the channel; the engine would refuse the take-back.
  const writing = useStore((s) => Boolean(s.typing[message.channel]) || Object.values(s.streams).some((v) => v.channel === message.channel));
  const [selection, setSelection] = useState<string | null>(null);
  const [playing, setPlaying] = useState(false);
  const [menuAt, setMenuAt] = useState<{ x: number; y: number } | null>(null);
  const body = useRef<HTMLDivElement>(null);

  const mine = message.author.kind === "user";
  const name = authorName(message.author, t, names, user);
  const speech = useMemo(() => speechOnly(message.text, marker).trim(), [message.text, marker]);

  const captureSelection = () => {
    const sel = window.getSelection();
    const root = body.current;
    if (!sel || sel.isCollapsed || !root) return;
    if (!root.contains(sel.anchorNode) || !root.contains(sel.focusNode)) return;
    const text = sel.toString().trim();
    if (text) setSelection(text);
  };

  const play = async () => {
    setPlaying(true);
    const file = await attempt(api.commands.ttsPlay(message.id));
    if (!file) return setPlaying(false);
    const audio = new Audio(api.mediaUrl(file.path));
    audio.onended = audio.onerror = () => setPlaying(false);
    audio.play().catch(() => setPlaying(false));
  };

  const closeMenu = useCallback(() => setMenuAt(null), []);
  const menuItems = (): MenuItem[] => {
    const out: MenuItem[] = [];
    // A selection inside this message is what the user means to copy; otherwise the spoken text.
    const picked = window.getSelection()?.toString().trim();
    const text = picked && body.current?.contains(window.getSelection()?.anchorNode ?? null) ? picked : speech;
    if (text) out.push({ label: t("message.copy"), icon: "copy", onSelect: () => void navigator.clipboard?.writeText(text).catch(() => undefined) });
    if (mine && !writing) {
      out.push({
        label: t("message.resend"),
        icon: "reset",
        onSelect: () => {
          if (after > 0 && !window.confirm(t("message.resend.confirm", { count: after }))) return;
          void resend(message.channel, message.id);
        },
      });
    }
    if (message.cites.length > 0) {
      out.push({ label: t("message.sources", { count: message.cites.length }), icon: "cases", onSelect: () => dispatch({ type: "sources", message: message.id }) });
    }
    return out;
  };

  const label =
    first && !mine && !direct ? (
      <p className={`author hue-${hueOf(message.author)}`}>
        <span>{t("message.author", { name })}</span>
        {message.relayed && <span className="tag relayed">{t("message.relayed")}</span>}
        {message.confidence === "low" && <span className="tag thin">{t("confidence.low")}</span>}
      </p>
    ) : (
      (message.relayed || message.confidence === "low") && (
        <p className="author">
          {message.relayed && <span className="tag relayed">{t("message.relayed")}</span>}
          {message.confidence === "low" && <span className="tag thin">{t("confidence.low")}</span>}
        </p>
      )
    );

  const picture = message.attachment || message.picture_lost ? <Picture message={message} /> : null;

  const foot = (
    <div className="message-foot">
      {message.cites.length > 0 && (
        <button type="button" className="link sources-toggle" aria-expanded={sourcesOpen} onClick={() => dispatch({ type: "sources", message: message.id })}>
          {t("message.sources", { count: message.cites.length })}
        </button>
      )}
      <span className="spacer" />
      <span className="hover-actions">
        {mine && (
          <button
            type="button"
            className="link"
            disabled={writing}
            onClick={() => {
              if (after > 0 && !window.confirm(t("message.resend.confirm", { count: after }))) return;
              void resend(message.channel, message.id);
            }}
          >
            {t("message.resend")}
          </button>
        )}
        {message.note && (
          <a className="link" href={api.mediaUrl(message.note.path)} target="_blank" rel="noopener noreferrer">
            {t("message.note")}
          </a>
        )}
        {showPlay && message.author.kind === "person" && speech && (
          <button type="button" className="icon" aria-label={playing ? t("message.playing") : t("message.play")} disabled={playing} onClick={play}>
            ▷
          </button>
        )}
        {!mine && speech && (
          <button
            type="button"
            className="icon"
            aria-label={t("message.actions")}
            aria-expanded={selection !== null}
            onClick={() => setSelection((cur) => (cur === null ? speech : null))}
          >
            ⋯
          </button>
        )}
      </span>
      <time dateTime={message.at}>{when(message.at)}</time>
    </div>
  );

  return (
    <>
    <ActionLines author={message.author} actions={message.actions} />
    <article id={`m-${message.id}`} className={`message ${mine ? "from-user" : "from-other"}${first ? " first" : ""}${last ? " last" : ""}`} aria-label={name}
      onContextMenu={(e) => {
        if (menuItems().length === 0) return;
        e.preventDefault();
        setMenuAt({ x: e.clientX, y: e.clientY });
      }}
    >
      {!mine && <div className="message-avatar">{last && <SpeakerAvatar author={message.author} name={name} />}</div>}
      <div className="message-stack">
        <div ref={body} className="message-body" onMouseUp={captureSelection} onKeyUp={captureSelection}>
          {parts.map((b, i) => (
            <BubbleView key={i} b={b} label={i === 0 ? label : null} top={i === 0 ? picture : null} foot={i === parts.length - 1 ? foot : null} />
          ))}
          {parts.length === 0 && <BubbleView b={{ before: [], text: null, after: [] }} label={label} top={picture} foot={foot} />}
        </div>

        {sourcesOpen && <SourcesPanel cites={message.cites} onClose={() => dispatch({ type: "sources", message: message.id })} />}
        {selection !== null && <SelectionMenu excerpt={selection} message={message} onClose={() => setSelection(null)} />}
        {menuAt && <Menu at={menuAt} items={menuItems()} label={t("message.actions")} onClose={closeMenu} />}
      </div>
      {mine && <div className="message-avatar">{last && <UserAvatar />}</div>}
    </article>
    {message.author.kind !== "user" && <Choices actions={message.actions} message={message} />}
    </>
  );
});

/** A reply still being written: same layout, text so far. */
export function StreamView({ author, text, channel, first = true }: { author: Author; text: string; channel?: string; first?: boolean }) {
  const t = useT();
  const direct = useStore((s) => s.channels.find((c) => c.id === channel)?.kind === "direct");
  const names = useStore((s) => s.names);
  const user = useStore((s) => s.settings?.user_name ?? null);
  const marker = useStore((s) => s.wording.scene_marker);
  const parts = useMemo(() => bubbles(text, marker), [text, marker]);
  const name = authorName(author, t, names, user);
  return (
    <article className={`message from-other streaming last${first ? " first" : ""}`} aria-busy="true" aria-label={name}>
      <div className="message-avatar">
        <SpeakerAvatar author={author} name={name} />
      </div>
      <div className="message-stack">
        <div className="message-body">
          {parts.map((b, i) => (
            <BubbleView key={i} b={b} label={i === 0 && first && !direct ? <p className={`author hue-${hueOf(author)}`}>{t("message.author", { name })}</p> : null} foot={null} />
          ))}
          {parts.length === 0 && (
            <div className="bubble">
              <p className="author muted">
                {name} · {t("message.streaming")}
              </p>
            </div>
          )}
        </div>
      </div>
    </article>
  );
}
