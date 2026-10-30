import { memo, useCallback, useMemo, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import type { Author, Message, Quote, ToolAction } from "../api";
import { useT } from "../i18n";
import { actionText, authorName, choicesOf, nameList, when } from "../lib/format";
import { jumpTo } from "../lib/jump";
import { renderMarkdown } from "../lib/markdown";
import { highlightMentions } from "../lib/mention";
import { bubbles, isBlank, speechOnly } from "../lib/scene";
import { deliveryOf } from "../lib/ticks";
import { useActions, useApi, useDispatch, useStore } from "../store";
import { SourcesPanel } from "./Citation";
import { Avatar, HOST_ID, UserAvatar } from "../ui/Avatar";
import { Dialog } from "../ui/Dialog";
import { Icon } from "../ui/Icon";
import { Menu, type MenuItem } from "../ui/Menu";
import { Ticks } from "../ui/Ticks";

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
function Choices({ actions, message, lead }: { actions: ToolAction[]; message: Message; lead?: string }) {
  const t = useT();
  const acts = useActions();
  const api = useApi();
  const dispatch = useDispatch();
  const channels = useStore((s) => s.channels);
  // An invitation is spent once the person is in the group, whoever added them.
  const choices = useMemo(
    () =>
      choicesOf(actions).filter(
        (c) => c.kind !== "invite" || !channels.find((x) => x.id === c.channel)?.participants.some((p) => p.id === c.id),
      ),
    [actions, channels],
  );
  if (choices.length === 0) return null;
  return (
    <div className="choices">
      {lead && <p className="service-line">{lead}</p>}
      {choices.map((c) =>
        c.kind === "invite" ? (
          <button
            key={`i:${c.channel}:${c.id}`}
            type="button"
            className="ghost"
            onClick={async () => {
              if ((await acts.attempt(api.commands.openInvite(message.id, c.action, c.id))) === undefined) return;
              await acts.refreshChannels();
            }}
          >
            {t("choice.invite", { name: c.name })}
          </button>
        ) : c.kind === "person" ? (
          <button key={`p:${c.id}`} type="button" className="ghost" onClick={() => (c.found === undefined ? acts.openDirect(c.id) : acts.openFound(message.id, c.found, c.id, c.name))}>
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
              await acts.open(group, c.members.map((id, i) => ({ id, name: c.names[i] ?? id })));
            }}
          >
            {t("choice.new_group", { names: nameList(c.names, t) })}
          </button>
        ) : (
          <button key={`g:${c.channel}`} type="button" className="ghost" onClick={() => acts.open(c.channel, c.members)}>
            {t("choice.group", { names: nameList(c.names, t) })}
          </button>
        ),
      )}
    </div>
  );
}

/** `names` are the members a message may address with `@name`; those are set apart in the accent colour. */
function Speech({ text, mentions = NO_NAMES }: { text: string; mentions?: string[] }) {
  const html = useMemo(() => highlightMentions(renderMarkdown(text), mentions), [text, mentions]);
  return <div className="speech" dangerouslySetInnerHTML={{ __html: html }} />;
}

const NO_NAMES: string[] = [];

/** A stable accent per speaker, so a name keeps its colour across the conversation. */
const HUES = 8;
export function hueOf(a: Author): number {
  const key = a.kind === "person" ? a.id : a.kind;
  let h = 0;
  for (const ch of key) h = (h * 31 + ch.charCodeAt(0)) % 997;
  return h % HUES;
}

/** The speaker's round avatar, shown beside the last bubble of a run; a person's opens their page. */
function SpeakerAvatar({ author, name }: { author: Author; name: string }) {
  const t = useT();
  const dispatch = useDispatch();
  if (author.kind === "user") return null;
  if (author.kind === "host") {
    return (
      <button type="button" className="avatar-button" aria-label={t("message.person.open", { name })} title={name} onClick={() => dispatch({ type: "hostCard", open: true })}>
        <Avatar person={{ id: HOST_ID, name }} size="m" />
      </button>
    );
  }
  return (
    <button type="button" className="avatar-button" aria-label={t("message.person.open", { name })} title={name} onClick={() => dispatch({ type: "personCard", person: author.id })}>
      <Avatar person={{ id: author.id, name }} size="m" />
    </button>
  );
}

/** A speaker's name above their bubble; a person's opens their page. */
function AuthorName({ author, name }: { author: Author; name: string }) {
  const t = useT();
  const dispatch = useDispatch();
  const text = t("message.author", { name });
  if (author.kind === "user") return <span>{text}</span>;
  return (
    <button type="button" className="author-name" onClick={() => dispatch(author.kind === "host" ? { type: "hostCard", open: true } : { type: "personCard", person: author.id })}>
      {text}
    </button>
  );
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

function BubbleView({ b, label, foot, top, mentions }: { b: ReturnType<typeof bubbles>[number]; label: ReactNode; foot: ReactNode; top?: ReactNode; mentions?: string[] }) {
  return (
    <div className="bubble">
      {label}
      {top}
      <SceneLines lines={b.before} />
      {b.text !== null && <Speech text={b.text} mentions={mentions} />}
      <SceneLines lines={b.after} />
      {foot}
    </div>
  );
}

/** What a reply answers, at the top of its bubble: the author in their colour and the start of the original. A click goes to it. */
function QuoteBlock({ quote }: { quote: Quote }) {
  const t = useT();
  const names = useStore((s) => s.names);
  const user = useStore((s) => s.settings?.user_name ?? null);
  const marker = useStore((s) => s.wording.scene_marker);
  const name = authorName(quote.author, t, names, user);
  const first = useMemo(
    () =>
      speechOnly(quote.text, marker)
        .split("\n")
        .map((l) => l.trim())
        .find(Boolean) ?? "",
    [quote.text, marker],
  );
  const shown = first || (quote.has_picture ? t("message.quote.picture") : "");
  return (
    <button type="button" className={`quote hue-${hueOf(quote.author)}`} aria-label={t("message.quote.open", { name })} onClick={() => jumpTo(quote.id)}>
      <span className="quote-name">{name}</span>
      <span className="quote-text">{shown}</span>
    </button>
  );
}

/**
 * One message: a run of bubbles (a paragraph each) with the speaker's avatar beside the
 * last one of the run. `first` and `last` say where this message sits in a run by the same
 * speaker; only the first carries the name, only the last the avatar.
 */
export const MessageView = memo(function MessageView(props: { message: Message; showPlay: boolean; first?: boolean; last?: boolean }) {
  return props.message.service ? <ServiceLine message={props.message} /> : <BubbleMessage {...props} />;
});

/** A line from the harness (why someone was found): a quiet note between the bubbles. */
function ServiceLine({ message }: { message: Message }) {
  return (
    <p className="service-line" id={`m-${message.id}`}>
      {message.text}
    </p>
  );
}

const BubbleMessage = memo(function BubbleMessage({
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
  const { attempt, resend, deleteSaved } = useActions();
  const names = useStore((s) => s.names);
  const user = useStore((s) => s.settings?.user_name ?? null);
  const marker = useStore((s) => s.wording.scene_marker);
  // A direct chat has one other speaker; the name only helps where several can talk.
  const kind = useStore((s) => s.channels.find((c) => c.id === message.channel)?.kind);
  const direct = kind === "direct";
  const favorites = kind === "favorites";
  // Whom a message may address with `@name`: the members of a group.
  const members = useStore((s) => s.channels.find((c) => c.id === message.channel)?.participants);
  const mentions = useMemo(() => (kind === "group" && members ? members.map((p) => p.name) : NO_NAMES), [kind, members]);
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
  const [playing, setPlaying] = useState(false);
  const [menuAt, setMenuAt] = useState<{ x: number; y: number } | null>(null);
  const wasOpen = useRef(false);
  const body = useRef<HTMLDivElement>(null);

  const mine = message.author.kind === "user";
  // Ticks on the user's own messages: one once a reply is under way, two when the round is done.
  const delivery = useStore((s) => (mine ? deliveryOf(s.messages[message.channel] ?? [], message.id, writing, Boolean(s.failures[message.channel])) : 0));
  const name = authorName(message.author, t, names, user);
  const speech = useMemo(() => speechOnly(message.text, marker).trim(), [message.text, marker]);

  const play = async () => {
    setPlaying(true);
    const file = await attempt(api.commands.ttsPlay(message.id));
    if (!file) return setPlaying(false);
    const audio = new Audio(api.mediaUrl(file.path));
    audio.onended = audio.onerror = () => setPlaying(false);
    audio.play().catch(() => setPlaying(false));
  };

  const closeMenu = useCallback(() => setMenuAt(null), []);
  const reply = () => {
    dispatch({ type: "reply", channel: message.channel, message });
    // The composer takes the focus when the reply bar appears.
    document.querySelector<HTMLTextAreaElement>(".composer textarea")?.focus();
  };
  const forward = () => dispatch({ type: "forwarding", message });
  const remove = () => {
    if (!window.confirm(t("message.delete.confirm"))) return;
    void deleteSaved(message.channel, message.id);
  };
  // The one home for what can be done to a message, in this order. Each has a key too, on the focused message.
  const menuItems = (): MenuItem[] => {
    const out: MenuItem[] = [];
    out.push({ label: t("message.reply"), icon: "reply", onSelect: reply });
    // A selection inside this message is what the user means to copy; otherwise the spoken text.
    const picked = window.getSelection()?.toString().trim();
    const text = picked && body.current?.contains(window.getSelection()?.anchorNode ?? null) ? picked : speech;
    if (text) out.push({ label: t("message.copy"), icon: "copy", onSelect: () => void navigator.clipboard?.writeText(text).catch(() => undefined) });
    out.push({ label: t("message.forward"), icon: "forward", onSelect: forward });
    // Taking back in the favorites would take the saved messages after it too.
    if (mine && !writing && !favorites) {
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
      out.push({ label: t("message.sources", { count: message.cites.length }), icon: "info", onSelect: () => dispatch({ type: "sources", message: message.id }) });
    }
    if (favorites) out.push({ label: t("message.delete"), icon: "trash", danger: true, separated: true, onSelect: remove });
    return out;
  };
  // Keys on the message itself, when it has the focus: R reply, F forward, Delete remove (favorites),
  // the menu key or Shift+F10 the whole menu.
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.target !== e.currentTarget || e.ctrlKey || e.metaKey || e.altKey) return;
    if ((e.key === "r" || e.key === "R") && !e.shiftKey) reply();
    else if ((e.key === "f" || e.key === "F") && !e.shiftKey) forward();
    else if (e.key === "Delete" && favorites) remove();
    else if (e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey)) {
      const r = e.currentTarget.getBoundingClientRect();
      setMenuAt({ x: r.left + 24, y: r.top + 24 });
    } else return;
    e.preventDefault();
  };

  // Who wrote a forwarded copy, then the name above a run: both sit at the top of the bubble.
  const forwarded = message.forwarded_from ? (
    <p className={`forwarded hue-${hueOf(message.forwarded_from)}`}>{t("message.forwarded", { name: authorName(message.forwarded_from, t, names, user) })}</p>
  ) : null;
  const label =
    first && !mine && !direct ? (
      <>
        <p className={`author hue-${hueOf(message.author)}`}>
          <AuthorName author={message.author} name={name} />
          {message.confidence === "low" && <span className="tag thin">{t("confidence.low")}</span>}
        </p>
        {forwarded}
      </>
    ) : (
      <>
        {message.confidence === "low" && (
          <p className="author">
            <span className="tag thin">{t("confidence.low")}</span>
          </p>
        )}
        {forwarded}
      </>
    );

  // The quote leads the bubble, above any picture.
  const picture =
    message.attachment || message.picture_lost || message.reply ? (
      <>
        {message.reply && <QuoteBlock quote={message.reply} />}
        {(message.attachment || message.picture_lost) && <Picture message={message} />}
      </>
    ) : null;

  const foot = (
    <div className="message-foot">
      {message.cites.length > 0 && (
        <button type="button" className="link sources-toggle" aria-expanded={sourcesOpen} onClick={() => dispatch({ type: "sources", message: message.id })}>
          {t("message.sources", { count: message.cites.length })}
        </button>
      )}
      <span className="spacer" />
      <span className="hover-actions">
        {showPlay && message.author.kind === "person" && speech && (
          <button type="button" className="icon" aria-label={playing ? t("message.playing") : t("message.play")} disabled={playing} onClick={play}>
            ▷
          </button>
        )}
        {(speech || (mine && !writing)) && (
          <button
            type="button"
            className="icon more"
            aria-label={t("message.actions")}
            title={t("message.actions")}
            aria-haspopup="menu"
            aria-expanded={menuAt !== null}
            // A press on the button while its menu is open closes the menu (as any press outside
            // it does); the click that follows must not open it again.
            onMouseDown={() => {
              wasOpen.current = menuAt !== null;
            }}
            onClick={(e) => {
              if (wasOpen.current) {
                wasOpen.current = false;
                return;
              }
              const r = e.currentTarget.getBoundingClientRect();
              setMenuAt({ x: r.left, y: r.bottom + 4 });
            }}
          >
            <Icon name="more" size={16} />
          </button>
        )}
      </span>
      {/* A kept message shows when it was first said; the list is in the order it was kept. */}
      <time dateTime={message.original_at ?? message.at}>{when(message.original_at ?? message.at, t)}</time>
      {mine && <Ticks delivery={delivery} />}
    </div>
  );

  // The sources unfold inside the bubble, above its foot.
  const withSources = (f: ReactNode) => (
    <>
      {sourcesOpen && <SourcesPanel cites={message.cites} onClose={() => dispatch({ type: "sources", message: message.id })} />}
      {f}
    </>
  );

  // Nothing to say and nothing to show: no bubble. The offers it carries, if any, stand
  // alone under a plain line.
  if (isBlank(message, marker)) {
    return (
      <>
        <ActionLines author={message.author} actions={message.actions} />
        {message.author.kind !== "user" && <Choices actions={message.actions} message={message} lead={t("message.offers")} />}
      </>
    );
  }

  return (
    <>
    <ActionLines author={message.author} actions={message.actions} />
    <article id={`m-${message.id}`} className={`message ${mine ? "from-user" : "from-other"}${first ? " first" : ""}${last ? " last" : ""}`} aria-label={name} tabIndex={0}
      onKeyDown={onKeyDown}
      onContextMenu={(e) => {
        if (menuItems().length === 0) return;
        e.preventDefault();
        setMenuAt({ x: e.clientX, y: e.clientY });
      }}
    >
      {!mine && <div className="message-avatar">{last && <SpeakerAvatar author={message.author} name={name} />}</div>}
      <div className="message-stack">
        <div ref={body} className="message-body">
          {parts.map((b, i) => (
            <BubbleView key={i} b={b} label={i === 0 ? label : null} top={i === 0 ? picture : null} foot={i === parts.length - 1 ? withSources(foot) : null} mentions={mentions} />
          ))}
          {parts.length === 0 && <BubbleView b={{ before: [], text: null, after: [] }} label={label} top={picture} foot={withSources(foot)} />}
        </div>

        {message.author.kind !== "user" && <Choices actions={message.actions} message={message} />}
        {menuAt && <Menu at={menuAt} items={menuItems()} label={t("message.actions")} onClose={closeMenu} />}
      </div>
      {mine && <div className="message-avatar">{last && <UserAvatar />}</div>}
    </article>
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
