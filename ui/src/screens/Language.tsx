import { useId } from "react";
import type { InterfaceLanguage } from "../api";
import { useT } from "../i18n";
import { useActions, useStore } from "../store";
import { Choices, Note, Row, Section } from "../ui/SettingsKit";

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
    <Choices label={legend}>
      {CHOICES.map((c) => (
        <Row key={c.value} kind="choice" group={name} checked={language === c.value} onChange={() => void actions.setInterfaceLanguage(c.value)} label={t(c.label)} />
      ))}
    </Choices>
  );
}

/** The language page of the settings card. */
export function LanguagePage() {
  const t = useT();
  return (
    <Section title={t("settings.page.language")}>
      <LanguageChoice legend={t("settings.page.language")} />
      <Note>{t("lang.hint")}</Note>
    </Section>
  );
}
