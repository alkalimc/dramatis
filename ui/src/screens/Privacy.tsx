import { useCallback, useEffect, useState } from "react";
import { settle, type Failure, type SaveInfo } from "../api";
import { useT } from "../i18n";
import { useActions, useApi } from "../store";
import { Icon, type IconName } from "../ui/Icon";
import { PasswordError, UnderlineField } from "./Unlock";

type View = "root" | "manage" | "on" | "change" | "off";

function Row({ icon, label, value, onClick }: { icon: IconName; label: string; value?: string; onClick: () => void }) {
  return (
    <li>
      <button type="button" className="settings-row" onClick={onClick}>
        <Icon name={icon} size={24} />
        <span className="settings-label">{label}</span>
        {value && <span className="settings-value">{value}</span>}
      </button>
    </li>
  );
}

/** Privacy and security: the open save's password, on or off, changed, removed. */
export function PrivacyPage() {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const [saves, setSaves] = useState<SaveInfo[] | null>(null);
  const [view, setView] = useState<View>("root");
  const [note, setNote] = useState("");

  const refresh = useCallback(async () => {
    const list = await actions.attempt(api.commands.saves());
    if (list) setSaves(list);
  }, [actions, api]);
  useEffect(() => {
    void refresh();
  }, [refresh]);

  const encrypted = saves?.find((s) => s.current)?.encrypted ?? false;
  const done = async (message: string) => {
    setNote(message);
    setView("root");
    await refresh();
  };

  if (view === "root" || view === "manage") {
    return (
      <section className="privacy">
        <h3 className="section-title">{t("privacy.section")}</h3>
        <ul className="settings-list">
          {view === "root" || !encrypted ? (
            <Row icon="lock" label={t("privacy.password")} value={saves ? (encrypted ? t("privacy.on") : t("privacy.off")) : undefined} onClick={() => setView(encrypted ? "manage" : "on")} />
          ) : (
            <>
              <Row icon="lock" label={t("privacy.change")} onClick={() => setView("change")} />
              <Row icon="trash" label={t("privacy.remove")} onClick={() => setView("off")} />
            </>
          )}
        </ul>
        <p className="muted small privacy-hint">{t("privacy.scope")}</p>
        <p role="status" className="muted privacy-note">
          {note}
        </p>
      </section>
    );
  }
  return (
    <PasswordForm
      key={view}
      mode={view}
      back={() => setView(encrypted ? "manage" : "root")}
      onDone={(m) => done(t(m))}
    />
  );
}

function PasswordForm({ mode, back, onDone }: { mode: "on" | "change" | "off"; back: () => void; onDone: (message: "privacy.done.on" | "privacy.done.change" | "privacy.done.off") => Promise<void> }) {
  const t = useT();
  const api = useApi();
  const [old, setOld] = useState("");
  const [next, setNext] = useState("");
  const [again, setAgain] = useState("");
  const [understood, setUnderstood] = useState(false);
  const [error, setError] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);

  const needsOld = mode !== "on";
  const needsNew = mode !== "off";
  const mismatch = needsNew && again !== "" && next !== again;
  const ready = (!needsOld || old !== "") && (!needsNew || (next !== "" && next === again)) && (mode !== "on" || understood);

  return (
    <form
      className="privacy-form"
      onSubmit={async (e) => {
        e.preventDefault();
        if (!ready) return;
        setBusy(true);
        const r = await settle(api.commands.setPassword(needsOld ? old : null, needsNew ? next : null));
        setBusy(false);
        if (r.ok) await onDone(mode === "on" ? "privacy.done.on" : mode === "change" ? "privacy.done.change" : "privacy.done.off");
        else {
          setError(r.error);
          setOld("");
        }
      }}
    >
      <button type="button" className="link" onClick={back}>
        {t("chatlist.back")}
      </button>
      <h3>{t(`privacy.${mode}.title`)}</h3>
      <p className="muted">{t(`privacy.${mode}.hint`)}</p>
      {mode === "on" && (
        <label className="check-row">
          <input type="checkbox" checked={understood} onChange={(e) => setUnderstood(e.target.checked)} />
          <span>{t("privacy.forget.confirm")}</span>
        </label>
      )}
      {needsOld && <UnderlineField label={t("privacy.current")} value={old} onChange={setOld} autoFocus />}
      {needsNew && <UnderlineField label={t("privacy.new")} value={next} onChange={setNext} autoComplete="new-password" autoFocus={!needsOld} />}
      {needsNew && <UnderlineField label={t("privacy.again")} value={again} onChange={setAgain} autoComplete="new-password" />}
      {mismatch && (
        <p role="alert" className="inline-error">
          {t("privacy.mismatch")}
        </p>
      )}
      <PasswordError error={error} />
      <button type="submit" className="wide" disabled={busy || !ready}>
        {t("privacy.submit")}
      </button>
    </form>
  );
}
