import { useCallback, useEffect, useId, useState } from "react";
import { settle, type Failure, type SaveInfo } from "../api";
import { useT } from "../i18n";
import { when } from "../lib/format";
import { useActions, useApi, useDispatch, useStore } from "../store";
import { Dialog } from "../ui/Dialog";
import { Icon } from "../ui/Icon";
import { PasswordDialog } from "./Unlock";

/** A folder's full path, selectable, with a button that copies it (there is no file manager to open from here). */
export function FolderNote({ text, path }: { text: string; path: string }) {
  const t = useT();
  const [copied, setCopied] = useState(false);
  return (
    <p className="folder-note muted small">
      {text} <code className="raw">{path}</code>{" "}
      <button
        type="button"
        className="link"
        onClick={() => {
          void Promise.resolve(navigator.clipboard?.writeText(path)).then(() => setCopied(true), () => setCopied(false));
        }}
      >
        {t("saves.folder.copy")}
      </button>
      <span role="status">{copied ? ` ${t("saves.folder.copied")}` : ""}</span>
    </p>
  );
}

type Prompt = { title: string; submit: (password: string) => Promise<Failure | null> };

/**
 * Open a save by name. An encrypted one not yet unlocked in this window asks for its
 * password first; `onSwitched` runs once it is open, and the window reloads by default.
 */
export function useOpenSave(onSwitched: () => void, refresh: () => Promise<void> = async () => undefined) {
  const t = useT();
  const api = useApi();
  const dispatch = useDispatch();
  const [prompt, setPrompt] = useState<Prompt | null>(null);
  const open = async (name: string) => {
    const r = await settle(api.commands.switchSave(name));
    if (r.ok) {
      await refresh();
      onSwitched();
    } else if (r.error.code === "locked") {
      setPrompt({
        title: t("saves.unlock.title", { name }),
        submit: async (password) => {
          const unlocked = await settle(api.commands.unlock(name, password));
          if (!unlocked.ok) return unlocked.error;
          const switched = await settle(api.commands.switchSave(name));
          if (!switched.ok) return switched.error;
          setPrompt(null);
          await refresh();
          onSwitched();
          return null;
        },
      });
    } else {
      dispatch({ type: "error", error: r.error });
    }
  };
  return { open, prompt, setPrompt };
}

type Ask = { kind: "copy" | "rename" | "export"; save: SaveInfo };

/**
 * System drawer, third tab: the saves of this home. Opening another one reloads the
 * window, since every list on screen belongs to the world that was open.
 */
export function SavesPage({ onSwitched = () => window.location.reload() }: { onSwitched?: () => void }) {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const dispatch = useDispatch();
  const id = useId();
  const status = useStore((s) => s.status);
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

  const { open, prompt, setPrompt } = useOpenSave(onSwitched, refresh);

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

  /** Import the file; an encrypted one asks for the password it was exported with. */
  const importFile = async (password: string | null): Promise<Failure | null> => {
    const r = await settle(api.commands.importSave(source.trim(), importName.trim() || null, password));
    if (r.ok) {
      setPrompt(null);
      setNote(t("saves.imported", { name: r.data }));
      setSource("");
      setImportName("");
      await refresh();
      return null;
    }
    if (r.error.code === "locked" && password === null) {
      setPrompt({ title: t("saves.import.password"), submit: importFile });
      return null;
    }
    if (password === null) dispatch({ type: "error", error: r.error });
    return r.error;
  };

  return (
    <section className="saves" aria-labelledby={`${id}-h`}>
      <h3 id={`${id}-h`}>{t("saves.title")}</h3>
      <p className="muted small">{t("saves.hint")}</p>
      {status && <FolderNote text={t("saves.folder")} path={status.saves_dir} />}
      {status?.save && (
        <p className="row wrap">
          <button type="button" className="ghost" onClick={() => void actions.logOut()}>
            <Icon name="leave" size={16} /> {t("logout")}
          </button>
          <span className="muted small">{t("logout.hint")}</span>
        </p>
      )}
      {saves?.length === 0 && <p className="muted">{t("saves.empty")}</p>}
      <ul className="save-list">
        {saves?.map((s) => (
          <li key={s.name} className={s.current ? "save current" : "save"}>
            <div className="save-main">
              <strong>{s.name}</strong>{" "}
              {s.current && <span className="badge">{t("saves.current")}</span>}{" "}
              {s.encrypted && (
                <span className="badge lock-badge">
                  <Icon name="lock" size={12} /> {t("saves.encrypted")}
                </span>
              )}
              <div className="muted small">
                {t("saves.size", { kb: s.size_kb })}
                {s.played_at && <> · {t("saves.played", { when: when(s.played_at, t) })}</>}
              </div>
            </div>
            <div className="row wrap" role="group" aria-label={t("saves.actions", { name: s.name })}>
              {!s.current && (
                <button
                  type="button"
                  onClick={() => void open(s.name)}
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
          await importFile(null);
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

      {prompt && <PasswordDialog title={prompt.title} submit={prompt.submit} onClose={() => setPrompt(null)} />}

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
