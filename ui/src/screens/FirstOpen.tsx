import { useId, useState } from "react";
import { useT } from "../i18n";
import { useActions, useStore } from "../store";
import { Credit } from "../ui/Credit";
import { LanguageChoice } from "./Language";
import { BIO_MAX } from "./Profile";
import { birthday, NameAndBirthday } from "./Quota";

/** First open: the language, then a name, a birthday and a bio, each optional, and straight into the app. */
export function FirstOpen() {
  const t = useT();
  const actions = useActions();
  const settings = useStore((s) => s.settings);
  const id = useId();
  const [name, setName] = useState("");
  const [month, setMonth] = useState("");
  const [day, setDay] = useState("");
  const [bio, setBio] = useState("");
  const [busy, setBusy] = useState(false);

  return (
    <main className="first-open">
      <form
        className="card"
        onSubmit={async (e) => {
          e.preventDefault();
          if (!settings || Array.from(bio.trim()).length > BIO_MAX) return;
          setBusy(true);
          await actions.saveSettings({ ...settings, user_name: name.trim() || null, birthday: birthday(month, day), bio: bio.trim() || null });
          setBusy(false);
        }}
      >
        <h1>{t("first.title")}</h1>
        <Credit />
        <LanguageChoice legend={t("first.language")} />
        <NameAndBirthday
          id={id}
          name={name}
          setName={setName}
          month={month}
          setMonth={setMonth}
          day={day}
          setDay={setDay}
          nameLabel={t("first.name")}
          birthdayLabel={t("first.birthday")}
          nameOptional
        />
        <label htmlFor={`${id}-bio`}>{t("first.bio")}</label>
        <textarea id={`${id}-bio`} rows={3} value={bio} onChange={(e) => setBio(e.target.value)} aria-describedby={`${id}-bio-hint`} />
        <p id={`${id}-bio-hint`} className="muted small">
          {t("profile.bio.hint")}
        </p>
        <p className="row end">
          <button type="submit" disabled={busy || Array.from(bio.trim()).length > BIO_MAX}>
            {t("first.submit")}
          </button>
        </p>
      </form>
    </main>
  );
}
