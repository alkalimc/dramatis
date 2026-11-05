import { useId } from "react";
import { useT, type T } from "../i18n";
import { isMac, type SendKey } from "../lib/keys";
import { useActions, useStore } from "../store";

const CHOICES: SendKey[] = ["mod", "enter"];

/** The key combination that sends when a bare Enter does not: Cmd+Enter on a Mac, Ctrl+Enter elsewhere. */
export const comboText = (t: T) => t(isMac() ? "chat.send.combo.mac" : "chat.send.combo.other");

/** Settings, typing page: which Enter sends a message, and the other one adds a line. */
export function ChatPage() {
  const t = useT();
  const actions = useActions();
  const sendKey = useStore((s) => s.sendKey);
  const name = useId();
  const combo = comboText(t);
  return (
    <section>
      <fieldset className="language">
        <legend>{t("chat.send.legend")}</legend>
        {CHOICES.map((c) => (
          <label key={c} className="switch">
            <input type="radio" name={name} checked={sendKey === c} onChange={() => actions.setSendKey(c)} />
            <span>{t(c === "enter" ? "chat.send.enter" : "chat.send.mod", { combo })}</span>
          </label>
        ))}
      </fieldset>
      <p className="muted small">{t("chat.send.hint")}</p>
    </section>
  );
}
