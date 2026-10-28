import { useCallback, useEffect, useId, useState } from "react";
import type { SaveInfo } from "../api";
import { useT } from "../i18n";
import { when } from "../lib/format";
import { useActions, useApi } from "../store";
import { Dialog } from "../ui/Dialog";

type Ask = { kind: "copy" | "rename" | "export"; save: SaveInfo };

/**
 * System drawer, third tab: the saves of this home. Opening another one reloads the
 * window, since every list on screen belongs to the world that was open.
 */
export function SavesPage({ onSwitched = () => window.location.reload() }: { onSwitched?: () => void }) {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const id = useId();
  const [saves, setSaves] = useState<SaveInfo[] | null>(null);
  const [ask, setAsk] = useState<Ask | null>(null);
  const [name, setName] = useState("");
  const [file, setFile] = useState("");
  const [fresh, setFresh] = useState("");
  const [source, setSource] = useState("");
  const [importName, setImportName] = useState("");
  const [note, setNote] = useState("");

  const refresh = useCallback(async () => {
    const list = await actions.attempt(api.commands.saves());
    if (list) setSaves(list);
  }, [actions, api]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  /** Run a command; on success refresh the list and say what happened if there is a note. */
  const run = async (p: ReturnType<typeof api.commands.saves> | Parameters<typeof actions.attempt>[0], done = "") => {
    const r = await actions.attempt(p);
    if (r === undefined) return false;
    setNote(done);
    await refresh();
    return true;
  };

  return (
    <section className="saves" aria-labelledby={`${id}-h`}>
      <h3 id={`${id}-h`}>{t("saves.title")}</h3>
      <p className="muted small">{t("saves.hint")}</p>
      {saves?.length === 0 && <p className="muted">{t("saves.empty")}</p>}
      <ul className="save-list">
        {saves?.map((s) => (
          <li key={s.name} className={s.current ? "save current" : "save"}>
            <div className="save-main">
              <strong>{s.name}</strong>{" "}
              {s.current && <span className="badge">{t("saves.current")}</span>}
              <div className="muted small">
                {t("saves.size", { kb: s.size_kb })}
                {s.played_at && <> · {t("saves.played", { when: when(s.played_at) })}</>}
              </div>
            </div>
            <div className="row wrap" role="group" aria-label={t("saves.actions", { name: s.name })}>
              {!s.current && (
                <button
                  type="button"
                  onClick={async () => {
                    if (await run(api.commands.switchSave(s.name))) onSwitched();
                  }}
                >
                  {t("saves.switch")}
                </button>
              )}
              {(["copy", "rename", "export"] as const).map((kind) => (
                <button
                  key={kind}
                  type="button"
                  className="ghost"
                  onClick={() => {
                    setAsk({ kind, save: s });
                    setName(kind === "copy" ? `${s.name} 2` : kind === "rename" ? s.name : "");
                    setFile("");
                  }}
                >
                  {t(`saves.${kind}`)}
                </button>
              ))}
              {!s.current && (
                <button
                  type="button"
                  className="ghost"
                  onClick={() => {
                    if (window.confirm(t("saves.delete.confirm", { name: s.name }))) void run(api.commands.deleteSave(s.name));
                  }}
                >
                  {t("saves.delete")}
                </button>
              )}
            </div>
          </li>
        ))}
      </ul>

      <form
        className="row wrap"
        onSubmit={async (e) => {
          e.preventDefault();
          if (fresh.trim() && (await run(api.commands.newSave(fresh.trim())))) setFresh("");
        }}
      >
        <label htmlFor={`${id}-new`}>{t("saves.new")}</label>
        <input id={`${id}-new`} value={fresh} placeholder={t("saves.name")} onChange={(e) => setFresh(e.target.value)} />
        <button type="submit" disabled={!fresh.trim()}>
          {t("saves.create")}
        </button>
      </form>

      <h4>{t("saves.import")}</h4>
      <form
        className="row wrap"
        onSubmit={async (e) => {
          e.preventDefault();
          if (!source.trim()) return;
          const added = await actions.attempt(api.commands.importSave(source.trim(), importName.trim() || null));
          if (added === undefined) return;
          setNote(t("saves.imported", { name: added }));
          setSource("");
          setImportName("");
          await refresh();
        }}
      >
        <input aria-label={t("saves.import.path")} placeholder={t("saves.import.path")} value={source} onChange={(e) => setSource(e.target.value)} />
        <input aria-label={t("saves.import.name")} placeholder={t("saves.import.name")} value={importName} onChange={(e) => setImportName(e.target.value)} />
        <button type="submit" disabled={!source.trim()}>
          {t("saves.import.go")}
        </button>
      </form>
      <p role="status" className="muted">
        {note}
      </p>

      {ask && (
        <Dialog title={t(`saves.${ask.kind}.title`, { name: ask.save.name })} onClose={() => setAsk(null)}>
          <form
            onSubmit={async (e) => {
              e.preventDefault();
              const value = ask.kind === "export" ? file.trim() : name.trim();
              if (!value) return;
              const call =
                ask.kind === "copy"
                  ? api.commands.copySave(ask.save.name, value)
                  : ask.kind === "rename"
                    ? api.commands.renameSave(ask.save.name, value)
                    : api.commands.exportSave(ask.save.name, value);
              if (await run(call)) setAsk(null);
            }}
          >
            {ask.kind === "export" ? (
              <>
                <label htmlFor={`${id}-to`}>{t("saves.export.path")}</label>
                <input id={`${id}-to`} value={file} onChange={(e) => setFile(e.target.value)} />
                <p className="muted small">{t("saves.export.hint")}</p>
              </>
            ) : (
              <>
                <label htmlFor={`${id}-to`}>{t(ask.kind === "copy" ? "saves.copy.to" : "saves.rename.to")}</label>
                <input id={`${id}-to`} value={name} onChange={(e) => setName(e.target.value)} />
              </>
            )}
            <p className="row end">
              <button type="button" className="ghost" onClick={() => setAsk(null)}>
                {t("saves.cancel")}
              </button>
              <button type="submit">{t("saves.ok")}</button>
            </p>
          </form>
        </Dialog>
      )}
    </section>
  );
}
