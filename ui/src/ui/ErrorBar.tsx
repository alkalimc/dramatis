import type { ApiError } from "../api";
import { useT, type T } from "../i18n";
import { when, type Names } from "../lib/format";
import { useDispatch, useStore } from "../store";

export function errorText(e: ApiError, t: T, names: Names): { text: string; detail?: string } {
  switch (e.code) {
    case "invalid":
      return { text: t("error.invalid", { field: e.field }) };
    case "disabled":
      return { text: t("error.disabled", { name: names[e.person] ?? t("person.unknown") }) };
    case "quota_exhausted":
      return { text: t("error.quota_exhausted", { when: when(e.release_at) }) };
    case "endpoint":
    case "keychain":
    case "io":
      return { text: t(`error.${e.code}`), detail: e.detail };
    default:
      return { text: t(`error.${e.code}`) };
  }
}

/** The last failed command, announced politely and dismissable. */
export function ErrorBar() {
  const t = useT();
  const error = useStore((s) => s.error);
  const names = useStore((s) => s.names);
  const dispatch = useDispatch();
  return (
    <div className="error-bar" role="status" aria-live="polite">
      {error && (
        <>
          <span>{errorText(error, t, names).text}</span>
          {errorText(error, t, names).detail && <code className="raw">{errorText(error, t, names).detail}</code>}
          <button type="button" className="link" onClick={() => dispatch({ type: "error", error: null })}>
            {t("drawer.close")}
          </button>
        </>
      )}
    </div>
  );
}
