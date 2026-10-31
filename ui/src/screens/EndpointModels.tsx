import { useId, useState } from "react";
import type { Model, WireApi } from "../api";
import { useT } from "../i18n";

type NumField = "context_window" | "cache_min_prefix" | "cache_breakpoints" | "price_in" | "price_in_cached" | "price_in_cache_write" | "price_out";
const NUM_FIELDS: NumField[] = ["context_window", "cache_min_prefix", "cache_breakpoints", "price_in", "price_in_cached", "price_in_cache_write", "price_out"];
// Cache write is only priced on the Anthropic-style wire.
const visible = (f: NumField, wire: WireApi) => f !== "price_in_cache_write" || wire === "messages";

/** Empty field = absent (the engine's defaults apply); anything else must be a number. */
function parseNum(raw: string): number | null | undefined {
  if (!raw.trim()) return null;
  const n = Number(raw);
  return Number.isFinite(n) && n >= 0 ? n : undefined;
}

/** The models of one profile by id, each with the limits and prices the engine may use. */
export function ModelList({ models, wire, duplicate, onChange }: { models: Model[]; wire: WireApi; duplicate: boolean; onChange: (next: Model[]) => void }) {
  const t = useT();
  const id = useId();
  const patch = (i: number, change: Partial<Model>) => onChange(models.map((m, j) => (j === i ? { ...m, ...change } : m)));

  return (
    <div className="model-editor">
      {models.length === 0 && <p className="muted small">{t("endpoints.models.empty")}</p>}
      {duplicate && (
        <p className="inline-error" role="alert">
          {t("error.invalid", { field: t("endpoints.model.id") })}
        </p>
      )}
      <ul className="model-list">
        {models.map((m, i) => (
          <li key={i}>
            <div className="row field-row">
              <label htmlFor={`${id}-m${i}`} className="sr-only">
                {t("endpoints.model.id")}
              </label>
              <input id={`${id}-m${i}`} value={m.id} placeholder={t("endpoints.model.untitled")} onChange={(e) => patch(i, { id: e.target.value })} />
              <button
                type="button"
                className="ghost danger"
                aria-label={t("endpoints.model.remove", { id: m.id || t("endpoints.model.untitled") })}
                onClick={() => onChange(models.filter((_, j) => j !== i))}
              >
                {t("endpoints.remove")}
              </button>
            </div>
            <details className="more">
              <summary>{t("endpoints.model.details")}</summary>
              <div className="field">
                <label htmlFor={`${id}-f${i}`}>{t("endpoints.model.forced_tool")}</label>
                <select
                  id={`${id}-f${i}`}
                  value={m.forced_tool == null ? "" : String(m.forced_tool)}
                  onChange={(e) => patch(i, { forced_tool: e.target.value === "" ? null : e.target.value === "true" })}
                >
                  <option value="">{t("endpoints.model.forced_tool.unknown")}</option>
                  <option value="true">{t("endpoints.model.forced_tool.yes")}</option>
                  <option value="false">{t("endpoints.model.forced_tool.no")}</option>
                </select>
              </div>
              {NUM_FIELDS.filter((f) => visible(f, wire)).map((f) => (
                <NumInput key={f} id={`${id}-${f}${i}`} label={t(`endpoints.model.${f}`)} value={m[f]} onChange={(v) => patch(i, { [f]: v })} />
              ))}
              <p className="muted small">{t("endpoints.model.prices.hint")}</p>
            </details>
          </li>
        ))}
      </ul>
      <p className="row">
        <button type="button" className="ghost" onClick={() => onChange([...models, { id: "" }])}>
          + {t("endpoints.model.add")}
        </button>
      </p>
    </div>
  );
}

/** A number field that keeps what is typed until it parses; empty means "use the default". */
function NumInput({ id, label, value, onChange }: { id: string; label: string; value: number | null | undefined; onChange: (v: number | null) => void }) {
  const [raw, setRaw] = useState(value == null ? "" : String(value));
  const invalid = parseNum(raw) === undefined;
  return (
    <div className="field">
      <label htmlFor={id}>{label}</label>
      <input
        id={id}
        inputMode="decimal"
        value={raw}
        aria-invalid={invalid}
        onChange={(e) => {
          setRaw(e.target.value);
          const v = parseNum(e.target.value);
          if (v !== undefined) onChange(v);
        }}
      />
    </div>
  );
}
