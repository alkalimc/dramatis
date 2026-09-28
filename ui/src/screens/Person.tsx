import { useEffect, useId, useState } from "react";
import type { PersonId } from "../api";
import { useT } from "../i18n";
import { when } from "../lib/format";
import { useQuery } from "../lib/useQuery";
import { useActions, useApi, useDispatch, useStore } from "../store";
import { Avatar } from "../ui/Avatar";
import { ModeSwitch } from "../ui/ModeSwitch";

/** One person: a single trust number, companions, mode, and a voice when read-aloud exists. */
export function PersonDrawer({ id }: { id: PersonId }) {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const dispatch = useDispatch();
  const version = useStore((s) => s.peopleVersion);
  const tts = useStore((s) => Boolean(s.endpoints?.roles?.tts));
  const { data: page } = useQuery(() => api.commands.personPage(id), [api, id, version]);

  if (!page) return <p className="muted drawer-body">{t("app.loading")}</p>;
  const off = page.mode === "disabled";

  return (
    <div className="drawer-body person-page">
      <button type="button" className="link" onClick={() => dispatch({ type: "drawer", drawer: { kind: "roster" } })}>
        ← {t("person.back")}
      </button>
      <header className="person-head">
        <Avatar person={page.person} size="l" />
        <div>
          <h2>{page.person.name}</h2>
          {page.summary && <p className="muted">{page.summary}</p>}
        </div>
        <p className="trust">{t("person.trust", { trust: page.trust })}</p>
      </header>
      <ModeSwitch
        mode={page.mode}
        hints="all"
        label={t("mode.label", { name: page.person.name })}
        onChange={(m) => actions.setMode({ kind: "person", id }, m)}
      />
      <p>
        {page.last_seen ? t("person.last_seen", { when: when(page.last_seen) }) : t("person.never")}
        {" · "}
        {t("person.tasks", { count: page.tasks_done })}
      </p>
      {page.companions.length > 0 && <p>{t("roster.companions", { names: page.companions.map((c) => c.name).join(" · ") })}</p>}
      {!off && (
        <p className="row">
          <button type="button" onClick={() => actions.openDirect(id)}>
            {t("person.talk")}
          </button>
          <button type="button" className="ghost" onClick={() => dispatch({ type: "ask", draft: { person: id } })}>
            {t("person.ask")}
          </button>
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
