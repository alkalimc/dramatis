import type { Failure } from "../api";
import { useT, type Key, type T } from "../i18n";
import { when, type Names } from "../lib/format";
import { useDispatch, useStore } from "../store";

/** Fields whose refusal has a sentence of its own. */
const INVALID: Record<string, Key> = {
  corpus: "error.invalid.corpus",
  corpus_version: "error.invalid.corpus_version",
  corpus_requirement: "error.invalid.corpus_requirement",
  base_url: "error.invalid.base_url",
  model: "error.invalid.model",
  host_model: "error.invalid.host_model",
};

export function errorText(e: Failure, t: T, names: Names): { text: string; detail?: string } {
  switch (e.code) {
    case "invalid":
      return { text: e.field in INVALID ? t(INVALID[e.field]) : t("error.invalid", { field: e.field }) };
    case "disabled":
      return { text: t("error.disabled", { name: names[e.person] ?? t("person.unknown") }) };
    case "quota_exhausted":
      return { text: t("error.quota_exhausted", { when: when(e.release_at, t) }) };
    case "endpoint":
    case "keychain":
    case "io":
    case "unknown":
      return { text: t(`error.${e.code}`), detail: e.detail };
    default:
      return { text: t(`error.${e.code}`) };
  }
}

/** One failure, calmly, in place; `onRetry` adds a retry button. */
export function InlineError({ error, onRetry }: { error: Failure; onRetry?: () => void }) {
  const t = useT();
  const names = useStore((s) => s.names);
  const shown = errorText(error, t, names);
  return (
    <p className="inline-error" role="status">
      <span>{shown.text}</span>
      {shown.detail && <code className="raw">{shown.detail}</code>}
      {onRetry && (
        <button type="button" className="link" onClick={onRetry}>
          {t("app.retry")}
        </button>
      )}
    </p>
  );
}

/** The last failed command, announced politely and dismissable. */
export function ErrorBar() {
  const t = useT();
  const error = useStore((s) => s.error);
  const names = useStore((s) => s.names);
  const dispatch = useDispatch();
  const shown = error ? errorText(error, t, names) : null;
  return (
    <div className="error-bar" role="status" aria-live="polite">
      {shown && (
        <>
          <span>{shown.text}</span>
          {shown.detail && <code className="raw">{shown.detail}</code>}
          <button type="button" className="link" onClick={() => dispatch({ type: "error", error: null })}>
            {t("app.dismiss")}
          </button>
        </>
      )}
    </div>
  );
}
