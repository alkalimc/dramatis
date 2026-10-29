import { useState } from "react";
import { settle, type Failure } from "../api";
import { useT } from "../i18n";
import { useActions, useApi } from "../store";
import { InlineError } from "../ui/ErrorBar";
import { useCurrentSave } from "./Profile";

/**
 * Another window or program replaced, renamed, deleted or encrypted the open save's file.
 * The engine has stopped writing to it; the person reopens it (which asks for the password
 * when the file is encrypted now) or picks another save.
 */
export function ChangedScreen() {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const save = useCurrentSave();
  const [error, setError] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);

  return (
    <main className="unlock">
      <div role="alert">
        <h1>{t("changed.title")}</h1>
        {save && <p className="muted">{t("unlock.save", { name: save })}</p>}
        <p>{t("changed.body")}</p>
        {error && <InlineError error={error} />}
        <button
          type="button"
          className="wide"
          disabled={busy || !save}
          onClick={async () => {
            if (!save) return;
            setBusy(true);
            const r = await settle(api.commands.switchSave(save));
            if (r.ok) await actions.boot();
            else setError(r.error);
            setBusy(false);
          }}
        >
          {t("changed.reopen")}
        </button>
        <p className="center">
          <button type="button" className="link" onClick={() => void actions.logOut()}>
            {t("changed.pick")}
          </button>
        </p>
      </div>
    </main>
  );
}
