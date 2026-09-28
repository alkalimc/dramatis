import { useId } from "react";
import type { Mode } from "../api";
import { useT, type Key } from "../i18n";

const MODES: Mode[] = ["enabled", "frozen", "disabled"];

export function modeHintKey(mode: Mode, kind: "person" | "group"): Key {
  return kind === "group" ? `mode.group.${mode}.hint` : `mode.${mode}.hint`;
}

/**
 * The three-state mode control: a radio group, so arrows move between states and the
 * current one is announced. `hints`: `selected` shows the current state's sentence,
 * `all` shows one sentence per state, `none` leaves them to the tooltip and the
 * accessible description (for dense lists that explain the modes once above).
 */
export function ModeSwitch({
  mode,
  onChange,
  label,
  kind = "person",
  hints = "selected",
  disabled = false,
}: {
  mode: Mode;
  onChange: (mode: Mode) => void;
  label: string;
  kind?: "person" | "group";
  hints?: "selected" | "all" | "none";
  disabled?: boolean;
}) {
  const t = useT();
  const id = useId();
  return (
    <fieldset className={`mode mode-${hints}`} disabled={disabled}>
      <legend className="sr-only">{label}</legend>
      <div className="mode-options">
        {MODES.map((m) => {
          const hint = t(modeHintKey(m, kind));
          const hintId = `${id}-${m}-hint`;
          return (
            <div className="mode-option" key={m}>
              <label className={`mode-choice mode-${m}`} title={hint}>
                <input
                  type="radio"
                  name={id}
                  value={m}
                  checked={mode === m}
                  aria-describedby={hintId}
                  onChange={() => onChange(m)}
                />
                <span>{t(`mode.${m}`)}</span>
              </label>
              <span id={hintId} className={hints === "all" ? "mode-hint" : "sr-only"}>
                {hint}
              </span>
            </div>
          );
        })}
      </div>
      {hints === "selected" && (
        <p className="mode-hint" aria-hidden="true">
          {t(modeHintKey(mode, kind))}
        </p>
      )}
    </fieldset>
  );
}
