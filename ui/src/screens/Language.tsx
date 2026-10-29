import { useId } from "react";
import type { InterfaceLanguage } from "../api";
import { useT } from "../i18n";
import { useActions, useStore } from "../store";

const CHOICES: { value: InterfaceLanguage; label: "lang.corpus" | "lang.english" }[] = [
  { value: "corpus", label: "lang.corpus" },
  { value: "english", label: "lang.english" },
];

/** The two interface languages; a choice applies at once, the screen included. */
export function LanguageChoice({ legend }: { legend: string }) {
  const t = useT();
  const actions = useActions();
  const language = useStore((s) => s.language);
  const name = useId();
  return (
    <fieldset className="language">
      <legend>{legend}</legend>
      {CHOICES.map((c) => (
        <label key={c.value} className="switch">
          <input type="radio" name={name} checked={language === c.value} onChange={() => void actions.setInterfaceLanguage(c.value)} />
          <span>{t(c.label)}</span>
        </label>
      ))}
    </fieldset>
  );
}

/** The language page of the settings card. */
export function LanguagePage() {
  const t = useT();
  return (
    <section>
      <LanguageChoice legend={t("settings.page.language")} />
      <p className="muted small">{t("lang.hint")}</p>
    </section>
  );
}
