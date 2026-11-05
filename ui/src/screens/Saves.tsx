import { useCallback, useEffect, useId, useRef, useState } from "react";
import { settle, type Failure, type SaveCheck, type SaveInfo, type VersionChange, type VersionStamp } from "../api";
import { useT } from "../i18n";
import { markFresh } from "../lib/fresh";
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

/** One side of a version comparison: what a save was last opened with, or what runs now. */
function StampLines({ stamp }: { stamp: VersionStamp }) {
  const t = useT();
  const unknown = t("saves.drift.unknown");
  return (
    <ul className="muted small">
      <li>{t("saves.drift.app", { version: stamp.app ?? unknown })}</li>
      <li>{t("saves.drift.pack", { name: stamp.pack ?? "", version: stamp.pack ? (stamp.pack_version ?? unknown) : unknown })}</li>
      <li>{t("saves.drift.schema", { n: stamp.schema })}</li>
    </ul>
  );
}

/** A command to run in a terminal, selectable, with a button that copies it. */
function Command({ text }: { text: string }) {
  const t = useT();
  const [copied, setCopied] = useState(false);
  return (
    <p>
      <code className="raw">{text}</code>{" "}
      <button
        type="button"
        className="link"
        onClick={() => {
          void Promise.resolve(navigator.clipboard?.writeText(text)).then(() => setCopied(true), () => setCopied(false));
        }}
      >
        {t("saves.migrate.copy")}
      </button>
      <span role="status">{copied ? ` ${t("saves.migrate.copied")}` : ""}</span>
    </p>
  );
}

/** A save in another data format is not loaded: it says what to run, or that the app is too old. */
function MigrationDialog({ check, onBack }: { check: SaveCheck; onBack: () => void }) {
  const t = useT();
  return (
    <Dialog title={t("saves.migrate.title", { name: check.name })} onClose={onBack}>
      <ul>
        {check.changes.map((c: VersionChange) => (
          <li key={c}>{t(`saves.drift.${c}`)}</li>
        ))}
      </ul>
      <StampLines stamp={check.saved} />
      {check.command ? (
        <>
          <p>{t("saves.migrate.body", { to: check.running.schema })}</p>
          <Command text={check.command} />
          <p className="muted small">{t("saves.migrate.protected")}</p>
        </>
      ) : (
        <p>{t("saves.migrate.newer")}</p>
      )}
      <div className="row">
        <button type="button" className="ghost" onClick={onBack}>
          {t("saves.migrate.close")}
        </button>
      </div>
    </Dialog>
  );
}

/** The warning before a save last opened with other versions is loaded. */
export function VersionDialog({ check, onLoad, onBack }: { check: SaveCheck; onLoad: () => void; onBack: () => void }) {
  const t = useT();
  if (check.changes.includes("needs_migration") || check.changes.includes("newer_schema")) return <MigrationDialog check={check} onBack={onBack} />;
  return (
    <Dialog title={t("saves.drift.title", { name: check.name })} onClose={onBack}>
      <div className="row wrap">
        <div>
          <strong>{t("saves.drift.saved")}</strong>
          <StampLines stamp={check.saved} />
        </div>
        <div>
          <strong>{t("saves.drift.running")}</strong>
          <StampLines stamp={check.running} />
        </div>
      </div>
      <ul>
        {check.changes.map((c: VersionChange) => (
          <li key={c}>{t(`saves.drift.${c}`)}</li>
        ))}
      </ul>
      <p className="muted small">{t("saves.drift.hint")}</p>
      <div className="row">
        <button type="button" onClick={onLoad}>
          {t("saves.drift.load")}
        </button>
        <button type="button" className="ghost" onClick={onBack}>
          {t("saves.drift.back")}
        </button>
      </div>
    </Dialog>
  );
}

/**
 * Open a save by name. The save's recorded versions are checked first: when they differ
 * from the running ones, `drift` holds the comparison until the user loads it anyway or
 * goes back. An encrypted save not yet unlocked in this window asks for its password
 * first; `onSwitched` runs once it is open, and the window reloads by default.
 */
export function useOpenSave(onSwitched: () => void, refresh: () => Promise<void> = async () => undefined) {
  const t = useT();
  const api = useApi();
  const dispatch = useDispatch();
  const [prompt, setPrompt] = useState<Prompt | null>(null);
  const [drift, setDrift] = useState<SaveCheck | null>(null);
  const load = async (name: string) => {
    setDrift(null);
    const r = await settle(api.commands.switchSave(name));
    if (r.ok) {
      await refresh();
      onSwitched();
    } else {
      dispatch({ type: "error", error: r.error });
    }
  };
  const open = async (name: string): Promise<void> => {
    const r = await settle(api.commands.checkSave(name));
    if (r.ok) {
      if (r.data.changes.length > 0) setDrift(r.data);
      else await load(name);
    } else if (r.error.code === "locked") {
      setPrompt({
        title: t("saves.unlock.title", { name }),
        submit: async (password) => {
          const unlocked = await settle(api.commands.unlock(name, password));
          if (!unlocked.ok) return unlocked.error;
          setPrompt(null);
          await open(name);
          return null;
        },
      });
    } else {
      dispatch({ type: "error", error: r.error });
    }
  };
  return { open, prompt, setPrompt, drift, load, back: () => setDrift(null) };
}

/** What a save's file says about it, for its card: versions and pack, or why they cannot be shown. */
function SaveVersions({ save }: { save: SaveInfo }) {
  const t = useT();
  const { stamp, state } = save;
  if (!stamp) return <div className="muted small save-versions">{t(state === "locked" ? "saves.card.locked" : "saves.card.unreadable")}</div>;
  const parts = [
    t("saves.card.app", { version: stamp.app ?? t("saves.drift.unknown") }),
    stamp.pack ? t("saves.card.pack", { name: stamp.pack, version: stamp.pack_version ?? "" }) : t("saves.card.nopack"),
    t("saves.card.schema", { n: stamp.schema }),
  ];
  return (
    <div className="muted small save-versions">
      {parts.join(" · ")}
      {state === "needs_migration" && (
        <>
          {" "}
          <span className="badge warn-badge">{t("saves.card.needs_migration")}</span>
        </>
      )}
      {state === "newer" && (
        <>
          {" "}
          <span className="badge warn-badge">{t("saves.card.newer")}</span>
        </>
      )}
    </div>
  );
}

type Ask = { kind: "copy" | "rename" | "export"; save: SaveInfo };

/**
 * System drawer, third tab: the saves of this home. Opening another one reloads the
 * window, since every list on screen belongs to the world that was open.
 *
 * `picker`: the same page with no save open (after logging out, or on a first start). It
 * leaves out what belongs to the drawer (title, hint, the log out button), a new save is
 * opened at once, and an imported one too.
 */
export function SavesPage({ onSwitched = () => window.location.reload(), picker = false, onCount }: { onSwitched?: () => void; picker?: boolean; onCount?: (n: number) => void }) {
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
    if (!list) return;
    setSaves(list);
    onCount?.(list.length);
  }, [actions, api, onCount]);

  const { open: openNamed, prompt, setPrompt, drift, load, back } = useOpenSave(onSwitched, refresh);
  // Saves made in this window: opening one starts the first-run steps, which can undo it.
  const made = useRef(new Set<string>());
  const open = async (name: string) => {
    if (made.current.has(name)) markFresh({ name, from: status?.save ?? null });
    await openNamed(name);
  };

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
      if (picker) {
        await open(r.data);
        return null;
      }
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
    <section className="saves" aria-labelledby={picker ? undefined : `${id}-h`}>
      {!picker && (
        <>
          <h3 id={`${id}-h`}>{t("saves.title")}</h3>
          <p className="muted small">{t("saves.hint")}</p>
        </>
      )}
      {status && <FolderNote text={t(picker ? "picker.folder" : "saves.folder")} path={status.saves_dir} />}
      {!picker && status?.save && (
        <p className="row wrap">
          <button type="button" className="ghost" onClick={() => void actions.logOut()}>
            <Icon name="leave" size={16} /> {t("logout")}
          </button>
          <span className="muted small">{t("logout.hint")}</span>
        </p>
      )}
      {saves?.length === 0 && !picker && <p className="muted">{t("saves.empty")}</p>}
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
              <SaveVersions save={s} />
            </div>
            <div className="row wrap" role="group" aria-label={t("saves.actions", { name: s.name })}>
              {!s.current && (
                <button type="button" aria-label={picker ? `${t("saves.switch")} ${s.name}` : undefined} onClick={() => void open(s.name)}>
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
          const given = (fresh.trim() || (picker ? t("picker.new.default") : "")).trim();
          if (!given) return;
          if (await run(api.commands.newSave(given))) {
            setFresh("");
            made.current.add(given);
            if (picker) await open(given);
          }
        }}
      >
        <label htmlFor={`${id}-new`}>{t(picker ? "picker.new.name" : "saves.new")}</label>
        <input id={`${id}-new`} value={fresh} placeholder={t(picker ? "picker.new.default" : "saves.name")} autoComplete="off" onChange={(e) => setFresh(e.target.value)} />
        <button type="submit" disabled={!picker && !fresh.trim()}>
          {t(picker ? "picker.new.go" : "saves.create")}
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
      {drift && <VersionDialog check={drift} onLoad={() => void load(drift.name)} onBack={back} />}

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
