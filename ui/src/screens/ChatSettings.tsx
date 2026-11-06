import { useId } from "react";
import { useT, type T } from "../i18n";
import { isMac, type SendKey } from "../lib/keys";
import type { Theme } from "../store";
import { useActions, useStore } from "../store";
import { Choices, Note, Row, Section } from "../ui/SettingsKit";

const CHOICES: SendKey[] = ["mod", "enter"];
const THEMES: Theme[] = ["system", "light", "dark"];

/** The key combination that sends when a bare Enter does not: Cmd+Enter on a Mac, Ctrl+Enter elsewhere. */
export const comboText = (t: T) => t(isMac() ? "chat.send.combo.mac" : "chat.send.combo.other");

/** Chat settings, typing page: which Enter sends a message, and the other one adds a line. */
export function InputPage() {
  const t = useT();
  const actions = useActions();
  const sendKey = useStore((s) => s.sendKey);
  const name = useId();
  const combo = comboText(t);
  return (
    <Section title={t("chat.send.legend")}>
      <Choices label={t("chat.send.legend")}>
        {CHOICES.map((c) => (
          <Row key={c} kind="choice" group={name} checked={sendKey === c} onChange={() => actions.setSendKey(c)} label={t(c === "enter" ? "chat.send.enter" : "chat.send.mod", { combo })} />
        ))}
      </Choices>
      <Note>{t("chat.send.hint")}</Note>
    </Section>
  );
}

/** Chat settings, night mode page: follow the system, or stay light or dark. */
export function NightPage() {
  const t = useT();
  const actions = useActions();
  const theme = useStore((s) => s.theme);
  const name = useId();
  return (
    <Section title={t("menu.night")}>
      <Choices label={t("menu.night")}>
        {THEMES.map((c) => (
          <Row key={c} kind="choice" group={name} checked={theme === c} onChange={() => actions.setTheme(c)} label={t(`night.${c}`)} hint={c === "system" ? t("night.system.hint") : undefined} />
        ))}
      </Choices>
    </Section>
  );
}
