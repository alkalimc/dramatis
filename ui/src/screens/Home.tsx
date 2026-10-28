import { useState } from "react";
import { useStore, useActions } from "../store";
import { useT } from "../i18n";
import { activityText } from "../lib/format";
import { Avatar } from "../ui/Avatar";
import { ChannelView } from "./Channel";
import { ColdstartCard } from "./Coldstart";
import { Library } from "./Library";
import { TypingMark } from "./Roster";

function TypingIn({ channel }: { channel: string }) {
  const on = useStore((s) => Boolean(s.typing[channel]));
  return on ? <TypingMark /> : null;
}

/** Rows shown before "show more": the panel sits above the conversation and must stay short. */
const WAITING_SHOWN = 2;

/** Pure data, always there, no model call: who has something for the user. */
export function WaitingPanel() {
  const t = useT();
  const actions = useActions();
  const items = useStore((s) => s.digest.items);
  const [all, setAll] = useState(false);
  const shown = all ? items : items.slice(0, WAITING_SHOWN);
  return (
    <section className="waiting" aria-labelledby="waiting-title">
      <h2 id="waiting-title">{t("waiting.title")}</h2>
      {items.length === 0 ? (
        <p className="muted">{t("waiting.empty")}</p>
      ) : (
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
      )}
      {items.length > WAITING_SHOWN && (
        <button type="button" className="link waiting-more" aria-expanded={all} onClick={() => setAll(!all)}>
          {all ? t("waiting.less") : t("waiting.more", { count: items.length - WAITING_SHOWN })}
        </button>
      )}
    </section>
  );
}

/** The main pane: the host channel with the waiting panel, or another channel. */
export function Main() {
  const form = useStore((s) => s.status?.form);
  const host = useStore((s) => s.hostChannel);
  const channel = useStore((s) => s.channel);
  const hostDone = useStore((s) => (host ? (s.historyDone[host] ?? false) : false));

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
