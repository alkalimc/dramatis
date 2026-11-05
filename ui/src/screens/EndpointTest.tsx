import { useEffect, useState } from "react";
import { settle, type EndpointTest, type Failure } from "../api";
import { useT } from "../i18n";
import { useApi } from "../store";
import { InlineError } from "../ui/ErrorBar";

/**
 * One short test message to a saved profile's model, and what came of it. `prepare` runs
 * first when the editor has unsaved changes to write; it answers whether to go on.
 * `onNetwork` adds a way to the network settings when the failure may be the route.
 */
export function EndpointTester({
  profile,
  model,
  label,
  prepare,
  onNetwork,
}: {
  profile: string;
  model: string;
  /** Names the button for a screen reader when several stand side by side. */
  label?: string;
  prepare?: () => Promise<boolean>;
  onNetwork?: () => void;
}) {
  const t = useT();
  const api = useApi();
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<EndpointTest | null>(null);
  const [error, setError] = useState<Failure | null>(null);

  // A result belongs to the model it was got from.
  useEffect(() => {
    setResult(null);
    setError(null);
  }, [profile, model]);

  const run = async () => {
    setBusy(true);
    setResult(null);
    setError(null);
    if (prepare && !(await prepare())) return setBusy(false);
    const r = await settle(api.commands.testEndpoint(profile, model));
    if (r.ok) setResult(r.data);
    else setError(r.error);
    setBusy(false);
  };

  const route = result && !result.ok && (result.failure === "address" || result.failure === "timeout");
  return (
    <div className="endpoint-test">
      <button type="button" className="ghost" disabled={busy || !model.trim()} aria-busy={busy} aria-label={label} title={t("endpoints.test.hint")} onClick={() => void run()}>
        {busy ? t("endpoints.test.busy") : t("endpoints.test")}
      </button>
      <div role="status" aria-live="polite">
        {result?.ok && <p className="small">{result.reply ? t("endpoints.test.ok", { ms: result.latency_ms, reply: result.reply }) : t("endpoints.test.ok.empty", { ms: result.latency_ms })}</p>}
        {result && !result.ok && result.failure && (
          <div className="fetch-error">
            <p>{t(`endpoints.test.fail.${result.failure}`)}</p>
            {result.detail && <code className="raw">{result.detail}</code>}
            {route && onNetwork && (
              <button type="button" className="link" onClick={onNetwork}>
                {t("endpoints.test.network")}
              </button>
            )}
          </div>
        )}
        {error && <InlineError error={error} />}
      </div>
    </div>
  );
}
