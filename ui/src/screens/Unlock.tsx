import { useId, useState, type ReactNode } from "react";
import { settle, type Failure } from "../api";
import { useT } from "../i18n";
import { useActions, useApi } from "../store";
import { Dialog } from "../ui/Dialog";
import { InlineError } from "../ui/ErrorBar";
import { useCurrentSave } from "./Profile";

/** A small teal label over a bare input with a teal underline. */
export function UnderlineField({
  label,
  value,
  onChange,
  autoComplete = "current-password",
  autoFocus = false,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  autoComplete?: string;
  autoFocus?: boolean;
}) {
  const id = useId();
  return (
    <div className="underline-field">
      <label htmlFor={id}>{label}</label>
      <input id={id} type="password" value={value} autoComplete={autoComplete} autoFocus={autoFocus} onChange={(e) => onChange(e.target.value)} />
    </div>
  );
}

/** What a password form says when a try fails: "wrong password" in place, anything else as it is. */
export function PasswordError({ error }: { error: Failure | null }) {
  return error ? <InlineError error={error} /> : null;
}

/** The open save is encrypted: nothing else shows until its password is given. */
export function UnlockScreen() {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const save = useCurrentSave();
  const [password, setPassword] = useState("");
  const [error, setError] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);

  return (
    <main className="unlock">
      <form
        onSubmit={async (e) => {
          e.preventDefault();
          if (!save || !password) return;
          setBusy(true);
          const r = await settle(api.commands.unlock(save, password));
          if (r.ok) {
            await actions.boot();
          } else {
            setError(r.error);
            setPassword("");
          }
          setBusy(false);
        }}
      >
        <h1>{t("unlock.title")}</h1>
        {save && <p className="muted">{t("unlock.save", { name: save })}</p>}
        <UnderlineField label={t("unlock.password")} value={password} onChange={setPassword} autoFocus />
        <PasswordError error={error} />
        <button type="submit" className="wide" disabled={busy || !save || !password}>
          {t("unlock.submit")}
        </button>
        <p className="center">
          <button type="button" className="link" onClick={() => void actions.logOut()}>
            {t("unlock.logout")}
          </button>
        </p>
      </form>
    </main>
  );
}

/** Ask for one password in a dialog; `submit` answers with the failure to show, or `null` when it worked. */
export function PasswordDialog({
  title,
  hint,
  submit,
  onClose,
}: {
  title: string;
  hint?: ReactNode;
  submit: (password: string) => Promise<Failure | null>;
  onClose: () => void;
}) {
  const t = useT();
  const [password, setPassword] = useState("");
  const [error, setError] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);
  return (
    <Dialog title={title} onClose={onClose}>
      <form
        className="password-dialog"
        onSubmit={async (e) => {
          e.preventDefault();
          if (!password) return;
          setBusy(true);
          const failed = await submit(password);
          setBusy(false);
          if (failed) {
            setError(failed);
            setPassword("");
          }
        }}
      >
        {hint && <p className="muted small">{hint}</p>}
        <UnderlineField label={t("unlock.password")} value={password} onChange={setPassword} autoFocus />
        <PasswordError error={error} />
        <p className="row end">
          <button type="button" className="ghost" onClick={onClose}>
            {t("saves.cancel")}
          </button>
          <button type="submit" disabled={busy || !password}>
            {t("unlock.submit")}
          </button>
        </p>
      </form>
    </Dialog>
  );
}
