import { useId, useState } from "react";
import { useT } from "../i18n";
import { useActions, useStore } from "../store";
import { birthday, NameAndBirthday } from "./Quota";

/** First open: a name, an optional birthday, and straight into the app. */
export function FirstOpen() {
  const t = useT();
  const actions = useActions();
  const settings = useStore((s) => s.settings);
  const id = useId();
  const [name, setName] = useState("");
  const [month, setMonth] = useState("");
  const [day, setDay] = useState("");
  const [busy, setBusy] = useState(false);

  return (
    <main className="first-open">
      <form
        className="card"
        onSubmit={async (e) => {
          e.preventDefault();
          if (!name.trim() || !settings) return;
          setBusy(true);
          await actions.saveSettings({ ...settings, user_name: name.trim(), birthday: birthday(month, day) });
          setBusy(false);
        }}
      >
        <h1>{t("first.title")}</h1>
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
        />
        <p className="row end">
          <button type="submit" disabled={busy || !name.trim()}>
            {t("first.submit")}
          </button>
        </p>
      </form>
    </main>
  );
}
