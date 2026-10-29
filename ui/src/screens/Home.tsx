import { useState } from "react";
import { useStore, useActions } from "../store";
import { useT } from "../i18n";
import { activityText } from "../lib/format";
import { Avatar } from "../ui/Avatar";
import { Icon } from "../ui/Icon";
import { ChannelView } from "./Channel";
import { ColdstartCard } from "./Coldstart";
import { Library } from "./Library";
import { TypingMark } from "./Roster";

function TypingIn({ channel }: { channel: string }) {
  const on = useStore((s) => Boolean(s.typing[channel]));
  return on ? <TypingMark /> : null;
}

/** Rows shown in the opened bar before "show more": the panel sits above the conversation. */
const WAITING_SHOWN = 2;

/** Pure data, always there, no model call: who has something for the user. A one-line pinned bar that opens into the list. */
export function WaitingPanel() {
  const t = useT();
  const actions = useActions();
  const items = useStore((s) => s.digest.items);
  const [open, setOpen] = useState(false);
  const [all, setAll] = useState(false);
  const shown = all ? items : items.slice(0, WAITING_SHOWN);
  const first = items[0];
  return (
    <section className="waiting" aria-labelledby="waiting-title">
      <div className="pinned-line" onClick={() => items.length > 0 && setOpen(!open)}>
        <h2 id="waiting-title">{t("waiting.title")}</h2>
        {first ? (
          <>
            <button
              type="button"
              className="pinned-first"
              onClick={(e) => {
                e.stopPropagation();
                void actions.open(first.channel, first.person);
              }}
            >
              <b>{first.person.name}</b> <span className="what">{activityText(first.activity, t)}</span>
            </button>
            {items.length > 1 && (
              <span className="pinned-count" aria-hidden="true">
                +{items.length - 1}
              </span>
            )}
            <button
              type="button"
              className="round small"
              aria-expanded={open}
              aria-label={t(open ? "waiting.collapse" : "waiting.expand")}
              title={t(open ? "waiting.collapse" : "waiting.expand")}
              onClick={(e) => {
                e.stopPropagation();
                setOpen(!open);
              }}
            >
              <Icon name="chevron" size={18} />
            </button>
          </>
        ) : (
          <span className="pinned-first muted">{t("waiting.empty")}</span>
        )}
      </div>
      {open && items.length > 0 && (
        <>
          <ul>
            {shown.map((w, i) => (
              <li key={`${w.person.id}-${w.activity.kind}-${i}`}>
                <button type="button" className="waiting-row" onClick={() => actions.open(w.channel, w.person)}>
                  <Avatar person={w.person} size="s" />
                  <span className="name">{w.person.name}</span>
                  <span className="what">{activityText(w.activity, t)}</span>
                  <TypingIn channel={w.channel} />
                </button>
              </li>
            ))}
          </ul>
          {items.length > WAITING_SHOWN && (
            <button type="button" className="link waiting-more" aria-expanded={all} onClick={() => setAll(!all)}>
              {all ? t("waiting.less") : t("waiting.more", { count: items.length - WAITING_SHOWN })}
            </button>
          )}
        </>
      )}
    </section>
  );
}

/** The main pane: the host channel with the waiting panel, or another channel. */
export function Main() {
  const form = useStore((s) => s.status?.form);
  const host = useStore((s) => s.hostChannel);
  const channel = useStore((s) => s.channel);
  const library = useStore((s) => s.library);
  const hostDone = useStore((s) => (host ? (s.historyDone[host] ?? false) : false));

  if (library) return <Library />;

  if (channel && channel !== host) {
    return (
      <div className="main">
        <ChannelView key={channel} channel={channel} />
      </div>
    );
  }

  if (form !== "full" || !host) return <Library />;

  return (
    <div className="main home">
      <ChannelView
        key={host}
        channel={host}
        pinned={<WaitingPanel />}
        // The first thing the host ever says is its introduction; the card belongs to it.
        after={(m, i) => (hostDone && i === 0 && m.author.kind === "host" ? <ColdstartCard /> : null)}
      />
    </div>
  );
}
