import { useEffect, useId, useState } from "react";
import type { PersonId } from "../api";
import { pronounOf, useT } from "../i18n";
import { when } from "../lib/format";
import { canTalk } from "../lib/library";
import { hasPersona } from "../lib/persona";
import { useQuery } from "../lib/useQuery";
import { useActions, useApi, useDispatch, useStore } from "../store";
import { Avatar, HOST_ID } from "../ui/Avatar";
import { Dialog } from "../ui/Dialog";
import { InlineError } from "../ui/ErrorBar";
import { Icon } from "../ui/Icon";
import { ModeSwitch } from "../ui/ModeSwitch";

/** A person's page over the conversation, opened from a name or avatar in a chat. */
export function PersonCard({ id }: { id: PersonId }) {
  const t = useT();
  const dispatch = useDispatch();
  const name = useStore((s) => s.names[id] ?? t("person.unknown"));
  const close = () => dispatch({ type: "personCard", person: null });
  return (
    <Dialog title={name} onClose={close} className="person-card" hideTitle>
      <button type="button" className="icon close" aria-label={t("drawer.close")} title={t("drawer.close")} onClick={close}>
        <Icon name="close" size={20} />
      </button>
      <PersonDrawer id={id} card />
    </Dialog>
  );
}

/** The host's page over the conversation, opened from its avatar or name in a chat. */
export function HostCard() {
  const t = useT();
  const dispatch = useDispatch();
  const close = () => dispatch({ type: "hostCard", open: false });
  return (
    <Dialog title={t("host.name")} onClose={close} className="person-card" hideTitle>
      <button type="button" className="icon close" aria-label={t("drawer.close")} title={t("drawer.close")} onClick={close}>
        <Icon name="close" size={20} />
      </button>
      <HostDrawer card />
    </Dialog>
  );
}

const HOST_CAN = ["host.can.find", "host.can.group", "host.can.ask", "host.can.explain"] as const;

/** The host's page, laid out like a person's: picture and name, one line about it, what it does, and the way to talk to it. */
export function HostDrawer({ card = false }: { card?: boolean }) {
  const t = useT();
  const actions = useActions();
  const dispatch = useDispatch();
  const hosted = useStore((s) => s.hostChannel !== null);
  const name = t("host.name");
  // Leaving this page for another: the card (if that is what this is) goes first.
  const leave = () => {
    dispatch({ type: "hostCard", open: false });
    dispatch({ type: "drawer", drawer: null });
  };
  const settings = (tab: "quota" | "about") => {
    dispatch({ type: "hostCard", open: false });
    dispatch({ type: "drawer", drawer: { kind: "system", tab } });
  };

  return (
    <div className={`drawer-body info-panel person-page host-page${card ? " in-card" : ""}`}>
      <header className="info-head">
        <Avatar person={{ id: HOST_ID, name }} size="xl" />
        {card ? <p className="info-name">{name}</p> : <h2 className="info-name">{name}</h2>}
        <p className="muted">{t("host.summary")}</p>
      </header>
      {hosted && (
        <p className="row info-buttons">
          <button
            type="button"
            onClick={() => {
              leave();
              void actions.open(null);
            }}
          >
            {t("person.talk")}
          </button>
        </p>
      )}
      <section className="info-section">
        <h3>{t("host.can")}</h3>
        <ul>
          {HOST_CAN.map((k) => (
            <li key={k}>{t(k)}</li>
          ))}
        </ul>
      </section>
      <section className="info-section">
        <p>
          <button type="button" className="link" onClick={() => settings("quota")}>
            {t("events.per_day")}
          </button>
        </p>
        <p>
          <button type="button" className="link" onClick={() => settings("about")}>
            {t("about.update.title")}
          </button>
        </p>
      </section>
    </div>
  );
}

/** One person: a single trust number, companions, mode, and a voice when read-aloud exists. */
export function PersonDrawer({ id, card = false }: { id: PersonId; card?: boolean }) {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const dispatch = useDispatch();
  const version = useStore((s) => s.peopleVersion);
  const talkable = useStore(canTalk);
  const tts = useStore((s) => Boolean(s.endpoints?.roles?.tts));
  const { data: page, error, reload } = useQuery(() => api.commands.personPage(id), [api, id, version]);

  if (!page) {
    return error ? (
      <div className="drawer-body">
        <InlineError error={error} onRetry={reload} />
      </div>
    ) : (
      <p className="muted drawer-body" role="status">
        {t("app.loading")}
      </p>
    );
  }
  const off = page.mode === "disabled";
  const ready = hasPersona(page);

  return (
    <div className={`drawer-body info-panel person-page${card ? " in-card" : ""}`}>
      {!card && (
        <button type="button" className="link back" onClick={() => dispatch({ type: "drawer", drawer: { kind: "roster" } })}>
          ← {t("person.back")}
        </button>
      )}
      <header className="info-head">
        <Avatar person={page.person} size="xl" />
        {/* In a card the dialog's title is the heading; the name here is what it shows. */}
        {card ? <p className="info-name">{page.person.name}</p> : <h2 className="info-name">{page.person.name}</h2>}
        {page.summary && <p className="muted">{page.summary}</p>}
        <p className="tag trust">{t("person.trust", { trust: page.trust })}</p>
      </header>
      {!off && talkable && (
        <p className="row info-buttons">
          <button type="button" disabled={!ready} aria-describedby={ready ? undefined : "unavailable-note"} onClick={() => actions.openDirect(id)}>
            {t("person.talk")}
          </button>
          <button
            type="button"
            className="ghost"
            disabled={!ready}
            aria-describedby={ready ? undefined : "unavailable-note"}
            onClick={() => dispatch({ type: "ask", draft: { person: id } })}
          >
            {t("person.ask", pronounOf(t, page.person))}
          </button>
        </p>
      )}
      <section className="info-section">
        <h3>{t("person.mode")}</h3>
        <ModeSwitch
          mode={page.mode}
          hints="all"
          label={t("mode.label", { name: page.person.name })}
          onChange={(m) => actions.setMode({ kind: "person", id }, m)}
        />
      </section>
      <section className="info-section">
        <p>
          {page.last_seen ? t("person.last_seen", { when: when(page.last_seen, t) }) : t("person.never")}
          {" · "}
          {t("person.tasks", { count: page.tasks_done, ...pronounOf(t, page.person) })}
        </p>
        {page.companions.length > 0 && <p>{t("roster.companions", { names: page.companions.map((c) => c.name).join(" · ") })}</p>}
      </section>
      {error && <InlineError error={error} onRetry={reload} />}
      {!ready && (
        <p className="muted unavailable" id="unavailable-note">
          <strong>{t("person.unavailable")}</strong> {t("person.unavailable.hint", pronounOf(t, page.person))}
        </p>
      )}
      {tts && <VoicePicker person={id} voice={page.voice} />}
    </div>
  );
}

function VoicePicker({ person, voice }: { person: PersonId; voice: string | null }) {
  const t = useT();
  const api = useApi();
  const { attempt } = useActions();
  const id = useId();
  const [voices, setVoices] = useState<string[] | null>(null);
  const [custom, setCustom] = useState(voice ?? "");

  useEffect(() => {
    void attempt(api.commands.ttsVoices()).then((v) => setVoices(v ?? []));
  }, [api, attempt]);

  const save = (v: string | null) => attempt(api.commands.setVoice(person, v));

  return (
    <fieldset className="voice">
      <legend>{t("person.voice")}</legend>
      {voices && voices.length > 0 && (
        <select
          aria-label={t("person.voice")}
          value={voices.includes(custom) ? custom : ""}
          onChange={(e) => {
            setCustom(e.target.value);
            void save(e.target.value || null);
          }}
        >
          <option value="">{t("person.voice.default")}</option>
          {voices.map((v) => (
            <option key={v} value={v}>
              {v}
            </option>
          ))}
        </select>
      )}
      <form
        className="row"
        onSubmit={(e) => {
          e.preventDefault();
          void save(custom.trim() || null);
        }}
      >
        <label htmlFor={id}>{t("person.voice.custom")}</label>
        <input id={id} value={custom} onChange={(e) => setCustom(e.target.value)} />
        <button type="submit" className="ghost">
          {t("person.voice.save")}
        </button>
      </form>
    </fieldset>
  );
}
