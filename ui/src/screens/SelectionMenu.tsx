import { useState } from "react";
import type { Candidate, ChannelId, Message } from "../api";
import { useT } from "../i18n";
import { useActions, useApi, useDispatch } from "../store";
import { CitationList } from "./Citation";
import { PersonPicker } from "./PersonPicker";

type Pane = "menu" | "where" | "relay" | "who";

/**
 * The one general verb: select a sentence, then do something with it. Everything here is
 * data (citations, roster search) except relaying, which is a real turn for the listener.
 */
export function SelectionMenu({ excerpt, message, onClose }: { excerpt: string; message: Message; onClose: () => void }) {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const dispatch = useDispatch();
  const [pane, setPane] = useState<Pane>("menu");
  const [who, setWho] = useState<Candidate[] | null>(null);
  const [relayed, setRelayed] = useState<{ name: string; channel: ChannelId } | null>(null);

  const findWho = async () => {
    setPane("who");
    const found = await actions.attempt(api.commands.findPeople(excerpt));
    setWho(found ?? []);
  };

  const relay = async (to: { id: string; name: string }) => {
    const channel = await actions.attempt(api.commands.relay(excerpt, message.id, to.id));
    if (channel) {
      setRelayed({ name: to.name, channel });
      void actions.refreshChannels();
    }
  };

  const check = () => {
    dispatch({ type: "ask", draft: { question: t("select.check.prefill", { excerpt }) } });
    onClose();
  };

  return (
    <section className="selection-menu" aria-label={t("select.label")} onKeyDown={(e) => e.key === "Escape" && onClose()}>
      <blockquote className="excerpt">
        <span className="sr-only">{t("select.whole")}: </span>
        {excerpt}
      </blockquote>
      <div className="row wrap" role="toolbar" aria-label={t("select.label")}>
        <button type="button" aria-pressed={pane === "where"} onClick={() => setPane("where")}>
          {t("select.where")}
        </button>
        <button type="button" aria-pressed={pane === "relay"} onClick={() => setPane("relay")}>
          {t("select.relay")}
        </button>
        <button type="button" onClick={check}>
          {t("select.check")}
        </button>
        <button type="button" aria-pressed={pane === "who"} onClick={findWho}>
          {t("select.who")}
        </button>
        <button type="button" className="link" onClick={onClose}>
          {t("select.close")}
        </button>
      </div>

      {pane === "where" &&
        (message.cites.length ? <CitationList cites={message.cites} /> : <p className="muted">{t("select.no_sources")}</p>)}

      {pane === "relay" &&
        (relayed ? (
          <p>
            {t("select.relay.sent", { name: relayed.name })}{" "}
            <button type="button" className="link" onClick={() => actions.open(relayed.channel)}>
              {t("select.relay.open")}
            </button>
          </p>
        ) : (
          <PersonPicker
            label={t("select.relay.pick")}
            exclude={message.author.kind === "person" ? [message.author.id] : []}
            onPick={relay}
          />
        ))}

      {pane === "who" &&
        (who === null ? (
          <p className="muted">{t("ask.searching")}</p>
        ) : who.length === 0 ? (
          <p className="muted">{t("select.who.empty")}</p>
        ) : (
          <ul className="candidates">
            {who.map((c) => (
              <li key={c.person.id}>
                <span className="name">{c.person.name}</span>
                <span className="reason">{c.reason}</span>
                <button type="button" className="link" onClick={() => dispatch({ type: "ask", draft: { question: excerpt, person: c.person.id } })}>
                  {t("person.ask")}
                </button>
              </li>
            ))}
          </ul>
        ))}
    </section>
  );
}
