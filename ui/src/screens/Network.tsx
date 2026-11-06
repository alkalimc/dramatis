import { useEffect, useId, useState } from "react";
import { settle, type Failure, type NetworkMode, type NetworkSetting } from "../api";
import { useT } from "../i18n";
import { useApi } from "../store";
import { InlineError } from "../ui/ErrorBar";
import { Body, Choices, Note, Row, Section } from "../ui/SettingsKit";

const MODES: NetworkMode[] = ["system", "direct", "manual"];

/**
 * How the app reaches the internet: this computer's proxy, none, or one typed here. Saved
 * with the button, applied to the next request. Settings and the first-run card share it.
 */
export function NetworkForm({ onSaved }: { onSaved?: () => void }) {
  const t = useT();
  const api = useApi();
  const id = useId();
  const [loaded, setLoaded] = useState(false);
  const [mode, setMode] = useState<NetworkMode>("system");
  const [proxy, setProxy] = useState("");
  const [username, setUsername] = useState("");
  const [noProxy, setNoProxy] = useState("");
  const [password, setPassword] = useState("");
  const [stored, setStored] = useState(false);
  const [replacing, setReplacing] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<Failure | null>(null);

  useEffect(() => {
    let live = true;
    void settle(api.commands.network()).then((r) => {
      if (!live) return;
      if (r.ok) {
        const s: NetworkSetting = r.data.setting;
        setMode(s.mode ?? "system");
        setProxy(s.proxy ?? "");
        setUsername(s.username ?? "");
        setNoProxy(s.no_proxy ?? "");
        setStored(r.data.has_password);
      }
      setLoaded(true);
    });
    return () => {
      live = false;
    };
  }, [api]);

  const manual = mode === "manual";
  const showPassword = !stored || replacing;

  const save = async () => {
    setBusy(true);
    setError(null);
    setSaved(false);
    // Nothing typed: leave the stored password; typed: replace it; removed: send empty.
    const next = removing ? "" : password ? password : null;
    const r = await settle(api.commands.setNetwork({ mode, proxy: proxy.trim(), username: username.trim(), no_proxy: noProxy.trim() }, next));
    setBusy(false);
    if (!r.ok) return setError(r.error);
    if (next !== null) setStored(next !== "");
    setPassword("");
    setReplacing(false);
    setRemoving(false);
    setSaved(true);
    onSaved?.();
  };

  if (!loaded) return <p className="muted" role="status">{t("app.loading")}</p>;

  return (
    <form
      className="network"
      noValidate
      onSubmit={(e) => {
        e.preventDefault();
        if (!busy) void save();
      }}
    >
      <Choices label={t("network.mode")}>
        {MODES.map((m) => (
          <Row key={m} kind="choice" group={`${id}-mode`} checked={mode === m} onChange={() => setMode(m)} label={t(`network.mode.${m}`)} hint={t(`network.mode.${m}.hint`)} />
        ))}
      </Choices>
      {manual && (
        <>
          <div className="field">
            <label htmlFor={`${id}-proxy`}>{t("network.proxy")}</label>
            <input id={`${id}-proxy`} value={proxy} placeholder="http://127.0.0.1:7890" autoComplete="off" spellCheck={false} aria-describedby={`${id}-proxy-h`} onChange={(e) => setProxy(e.target.value)} />
            <p id={`${id}-proxy-h`} className="muted small">
              {t("network.proxy.hint")}
            </p>
          </div>
          <div className="field">
            <label htmlFor={`${id}-user`}>{t("network.username")}</label>
            <input id={`${id}-user`} value={username} autoComplete="off" spellCheck={false} onChange={(e) => setUsername(e.target.value)} />
          </div>
          <div className="field">
            {showPassword ? <label htmlFor={`${id}-pw`}>{t("network.password")}</label> : <span>{t("network.password")}</span>}
            <div className="row field-row">
              {showPassword ? (
                <input id={`${id}-pw`} type="password" value={password} autoComplete="off" spellCheck={false} onChange={(e) => setPassword(e.target.value)} />
              ) : (
                <span className="key-stored">{removing ? t("network.password.remove") : t("network.password.stored")}</span>
              )}
              {!showPassword && !removing && (
                <>
                  <button type="button" className="ghost" onClick={() => setReplacing(true)}>
                    {t("network.password.replace")}
                  </button>
                  <button type="button" className="ghost" onClick={() => setRemoving(true)}>
                    {t("network.password.remove")}
                  </button>
                </>
              )}
              {!showPassword && removing && (
                <button type="button" className="ghost" onClick={() => setRemoving(false)}>
                  {t("endpoints.cancel")}
                </button>
              )}
            </div>
          </div>
          <div className="field">
            <label htmlFor={`${id}-skip`}>{t("network.no_proxy")}</label>
            <input id={`${id}-skip`} value={noProxy} autoComplete="off" spellCheck={false} aria-describedby={`${id}-skip-h`} onChange={(e) => setNoProxy(e.target.value)} />
            <p id={`${id}-skip-h`} className="muted small">
              {t("network.no_proxy.hint")}
            </p>
          </div>
        </>
      )}
      {error && <InlineError error={error} />}
      <p className="row end">
        {saved && (
          <span className="muted small spacer" role="status">
            {t("network.saved")}
          </span>
        )}
        <button type="submit" disabled={busy} aria-busy={busy}>
          {t("network.save")}
        </button>
      </p>
    </form>
  );
}

/** The network page of the settings card. */
export function NetworkPage() {
  const t = useT();
  return (
    <Section title={t("network.mode")}>
      <Note>{t("network.intro")}</Note>
      <Body>
        <NetworkForm />
      </Body>
    </Section>
  );
}
