import { useEffect, useId, useState } from "react";
import { settle, type Failure, type HomeData, type HomeLocation } from "../api";
import { useT } from "../i18n";
import { useApi } from "../store";
import { InlineError } from "../ui/ErrorBar";
import { Body, Choices, Row } from "../ui/SettingsKit";

/**
 * Where the app keeps its files: the folder in use, and a way to move to another (or back
 * to the default), copying what is kept now or starting from what is in the new folder.
 * The old folder stays. The app restarts when the move is made. Settings and the first-run
 * cards share it.
 */
export function LocationForm({ compact = false }: { compact?: boolean }) {
  const t = useT();
  const api = useApi();
  const id = useId();
  const [where, setWhere] = useState<HomeLocation | null>(null);
  const [path, setPath] = useState("");
  const [data, setData] = useState<HomeData>("copy");
  const [note, setNote] = useState("");
  const [error, setError] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);
  const [moved, setMoved] = useState(false);

  useEffect(() => {
    let live = true;
    void settle(api.commands.homeLocation()).then((r) => {
      if (live && r.ok) setWhere(r.data);
    });
    return () => {
      live = false;
    };
  }, [api]);

  if (!where) return <p className="muted" role="status">{t("app.loading")}</p>;

  const choose = async () => {
    setNote("");
    const r = await settle(api.commands.chooseHomeFolder());
    if (!r.ok) return setNote(t("location.nodialog"));
    if (r.data) setPath(r.data);
  };

  const move = async (to: string | null) => {
    if (!window.confirm(to === null ? t("location.confirm.default") : t("location.confirm", { path: to }))) return;
    setBusy(true);
    setError(null);
    const r = await settle(api.commands.setHomeLocation(to, data));
    if (r.ok) return setMoved(true);
    setBusy(false);
    setError(r.error);
  };

  return (
    <section className="sx location" aria-labelledby={`${id}-h`}>
      {compact ? (
        <h3 id={`${id}-h`} className="sr-only">
          {t("location.title")}
        </h3>
      ) : (
        <>
          <h3 id={`${id}-h`} className="sx-title">
            {t("location.title")}
          </h3>
          <p className="sx-note">{t("location.intro")}</p>
        </>
      )}
      <Body>
      <p className="small">
        {t("location.now")}: <code className="raw">{where.path}</code>
      </p>
      <p className="muted small">{t(`location.source.${where.source}`)}</p>
      {where.unavailable && (
        <p role="alert" className="inline-error">
          {t("location.unavailable", { path: where.unavailable })}
        </p>
      )}
      {moved ? (
        <p role="status">{t("location.restarting")}</p>
      ) : (
        where.can_change && (
          <form
            noValidate
            onSubmit={(e) => {
              e.preventDefault();
              if (!busy && path.trim()) void move(path.trim());
            }}
          >
            <p className="row">
              <button type="button" className="ghost" onClick={() => void choose()}>
                {t("location.choose")}
              </button>
            </p>
            <div className="field">
              <label htmlFor={`${id}-path`}>{t("location.path")}</label>
              <input id={`${id}-path`} value={path} placeholder="~/dramatis" autoComplete="off" spellCheck={false} onChange={(e) => setPath(e.target.value)} />
            </div>
            {note && (
              <p className="muted small" role="status">
                {note}
              </p>
            )}
            <Choices label={t("location.data")}>
              {(["copy", "empty"] as const).map((d) => (
                <Row key={d} kind="choice" group={`${id}-data`} checked={data === d} onChange={() => setData(d)} label={t(`location.data.${d}`)} hint={t(`location.data.${d}.hint`)} />
              ))}
            </Choices>
            <p className="muted small">{t("location.keep")}</p>
            {error && (error.code === "exists" ? <p className="inline-error" role="status">{t("location.exists")}</p> : <InlineError error={error} />)}
            <p className="row end">
              {where.source !== "default" && (
                <button type="button" className="ghost" disabled={busy} onClick={() => void move(null)}>
                  {t("location.default")}
                </button>
              )}
              <button type="submit" disabled={busy || !path.trim()} aria-busy={busy}>
                {t("location.use")}
              </button>
            </p>
          </form>
        )
      )}
      </Body>
    </section>
  );
}
