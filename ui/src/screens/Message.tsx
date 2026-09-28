import { memo, useMemo, useRef, useState } from "react";
import type { Author, Citation, Message, ToolAction } from "../api";
import { useT } from "../i18n";
import { actionText, authorName, when } from "../lib/format";
import { renderMarkdown } from "../lib/markdown";
import { splitScene } from "../lib/scene";
import { useActions, useApi, useStore } from "../store";
import { CitationDetail, CitationList, useCitations } from "./Citation";
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

/**
 * Split a message's citations: the speaker's own material (a plain bubble with sources
 * folded underneath) and material from the archive at large (shown as quote cards).
 */
function useSources(author: Author, cites: Citation[]) {
  const views = useCitations(cites);
  return useMemo(() => {
    const own: number[] = [];
    const scope: number[] = [];
    cites.forEach((_, i) => {
      const passage = views?.[i]?.passage;
      const mine = author.kind === "person" && passage?.persons.some((p) => p.id === author.id);
      (mine || !passage ? own : scope).push(i);
    });
    return { views, own, scope };
  }, [author, cites, views]);
}

function Speech({ text }: { text: string }) {
  const html = useMemo(() => renderMarkdown(text), [text]);
  return <div className="speech" dangerouslySetInnerHTML={{ __html: html }} />;
}

export const MessageView = memo(function MessageView({ message, showPlay }: { message: Message; showPlay: boolean }) {
  const t = useT();
  const api = useApi();
  const { attempt } = useActions();
  const names = useStore((s) => s.names);
  const user = useStore((s) => s.settings?.user_name ?? null);
  const marker = useStore((s) => s.wording.scene_marker);
  const segments = useMemo(() => splitScene(message.text, marker), [message.text, marker]);
  const { views, own, scope } = useSources(message.author, message.cites);
  const [sourcesOpen, setSourcesOpen] = useState(false);
  const [selection, setSelection] = useState<string | null>(null);
  const [playing, setPlaying] = useState(false);
  const body = useRef<HTMLDivElement>(null);

  const mine = message.author.kind === "user";
  const name = authorName(message.author, t, names, user);
  const speech = segments.filter((s) => s.kind === "speech").map((s) => s.text).join("\n");

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

  return (
    <article className={`message ${mine ? "from-user" : "from-other"}`} aria-label={name}>
      <ActionLines author={message.author} actions={message.actions} />
      <div ref={body} className="message-body" onMouseUp={captureSelection} onKeyUp={captureSelection}>
        {segments.map((s, i) =>
          s.kind === "scene" ? (
            <p key={i} className="scene">
              {s.text}
            </p>
          ) : (
            <div key={i} className="bubble">
              <p className="author">
                <span>{t("message.author", { name })}</span>
                {message.relayed && <span className="tag relayed">{t("message.relayed")}</span>}
                {message.confidence === "low" && <span className="tag thin">{t("confidence.low")}</span>}
              </p>
              <Speech text={s.text} />
            </div>
          ),
        )}
        {message.attachment && <img className="attachment" src={api.mediaUrl(message.attachment.path)} alt="" />}
      </div>

      {scope.length > 0 && (
        <div className="quote-cards">
          {scope.map((i) => (
            <figure key={i} className="quote-card">
              <figcaption>{t("citation.label")}</figcaption>
              <CitationDetail citation={message.cites[i]} view={views ? views[i] : null} />
            </figure>
          ))}
        </div>
      )}

      <div className="message-foot">
        <time dateTime={message.at}>{when(message.at)}</time>
        {own.length > 0 && (
          <button type="button" className="link" aria-expanded={sourcesOpen} onClick={() => setSourcesOpen((v) => !v)}>
            {t("message.sources", { count: own.length })}
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
      </div>

      {sourcesOpen && <CitationList cites={own.map((i) => message.cites[i])} />}
      {selection !== null && <SelectionMenu excerpt={selection} message={message} onClose={() => setSelection(null)} />}
    </article>
  );
});

/** A reply still being written: same layout, text so far. */
export function StreamView({ author, text }: { author: Author; text: string }) {
  const t = useT();
  const names = useStore((s) => s.names);
  const user = useStore((s) => s.settings?.user_name ?? null);
  const marker = useStore((s) => s.wording.scene_marker);
  const segments = useMemo(() => splitScene(text, marker), [text, marker]);
  const name = authorName(author, t, names, user);
  return (
    <article className="message from-other streaming" aria-busy="true" aria-label={name}>
      <div className="message-body">
        {segments.map((s, i) =>
          s.kind === "scene" ? (
            <p key={i} className="scene">
              {s.text}
            </p>
          ) : (
            <div key={i} className="bubble">
              <p className="author">{t("message.author", { name })}</p>
              <Speech text={s.text} />
            </div>
          ),
        )}
        {segments.length === 0 && (
          <p className="author muted">
            {name} · {t("message.streaming")}
          </p>
        )}
      </div>
    </article>
  );
}
