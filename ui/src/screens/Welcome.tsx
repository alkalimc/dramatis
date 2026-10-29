import { useEffect, useId, useState } from "react";
import { settle, type ApiError, type CorpusState, type Failure, type SaveInfo, type WireApi } from "../api";
import { useT } from "../i18n";
import type { Key } from "../i18n";
import { when } from "../lib/format";
import { useActions, useApi, useStore } from "../store";
import { InlineError } from "../ui/ErrorBar";
import { Icon } from "../ui/Icon";
import { FolderNote, useOpenSave } from "./Saves";
import { PasswordDialog } from "./Unlock";

/** The first-run screens: the corpus, the save, the model. Each says what it needs and checks the answer. */

const reload = () => window.location.reload();

/** No corpus is open: choose its file (a dialog where there is one, else a typed path). */
export function CorpusSetup({ corpus }: { corpus: CorpusState }) {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const status = useStore((s) => s.status);
  const id = useId();
  const [path, setPath] = useState("");
  const [error, setError] = useState<Failure | null>(null);
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);

  const use = async (given: string) => {
    if (!given.trim()) return;
    setBusy(true);
    setError(null);
    const r = await settle(api.commands.setCorpus(given));
    if (r.ok) {
      // The app opens again on the corpus; outside it (the preview) the status says so.
      await actions.boot();
    } else {
      setError(r.error);
    }
    setBusy(false);
  };

  const choose = async () => {
    setNote("");
    const r = await settle(api.commands.chooseCorpus());
    if (!r.ok) return setNote(t("setup.corpus.nodialog"));
    if (r.data) {
      setPath(r.data);
      await use(r.data);
    }
  };

  const problem = corpus.state === "unsupported_version" ? t("app.corpus.unsupported", { version: corpus.format_version }) : corpus.state === "missing_requirement" ? t("app.corpus.requirement", { requirement: corpus.requirement }) : null;

  return (
    <main className="first-open setup">
      <form
        className="card"
        onSubmit={(e) => {
          e.preventDefault();
          void use(path);
        }}
      >
        <h1>{t("setup.corpus.title")}</h1>
        {problem ? <p role="alert">{problem}</p> : <p>{t("setup.corpus.intro")}</p>}
        <p className="row">
          <button type="button" className="ghost" onClick={() => void choose()}>
            {t("setup.corpus.choose")}
          </button>
        </p>
        <div className="underline-field">
          <label htmlFor={id}>{t("setup.corpus.path")}</label>
          <input id={id} value={path} placeholder="~/corpus/archive.folio" autoComplete="off" spellCheck={false} onChange={(e) => setPath(e.target.value)} />
        </div>
        {note && <p className="muted small" role="status">{note}</p>}
        {error && (error.code === "not_found" ? <p className="inline-error" role="status">{t("setup.corpus.notfound")}</p> : <InlineError error={error} />)}
        <p className="row end">
          <button type="submit" disabled={busy || !path.trim()}>
            {t("setup.corpus.use")}
          </button>
        </p>
        {status && <FolderNote text={t("setup.folder")} path={status.home_dir} />}
      </form>
    </main>
  );
}

/** No save is open: pick one, create the first, or import a file. Opening reloads the window onto it. */
export function SavePicker({ onSwitched = reload }: { onSwitched?: () => void }) {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const status = useStore((s) => s.status);
  const id = useId();
  const [saves, setSaves] = useState<SaveInfo[] | null>(null);
  const [fresh, setFresh] = useState("");
  const [source, setSource] = useState("");
  const [importName, setImportName] = useState("");
  const [error, setError] = useState<Failure | null>(null);
  const { open, prompt, setPrompt } = useOpenSave(onSwitched);

  // Loaded once; the list does not change while nothing is open.
  useEffect(() => {
    void actions.attempt(api.commands.saves()).then((list) => setSaves(list ?? []));
  }, [actions, api]);

  const create = async () => {
    const name = (fresh.trim() || t("picker.new.default")).trim();
    setError(null);
    const r = await settle(api.commands.newSave(name));
    if (!r.ok) return setError(r.error);
    await open(name);
  };

  const importFile = async (password: string | null): Promise<Failure | null> => {
    const r = await settle(api.commands.importSave(source.trim(), importName.trim() || null, password));
    if (r.ok) {
      setPrompt(null);
      await open(r.data);
      return null;
    }
    if (r.error.code === "locked" && password === null) {
      setPrompt({ title: t("saves.import.password"), submit: importFile });
      return null;
    }
    if (password === null) setError(r.error);
    return r.error;
  };

  const first = saves !== null && saves.length === 0;
  const missing = status?.missing_save;

  return (
    <main className="first-open setup picker">
      <div className="card">
        <h1>{first ? t("picker.first") : t("picker.title")}</h1>
        {missing && <p role="alert">{t("picker.missing", { name: missing })}</p>}
        {first && <p>{t("picker.first.hint")}</p>}
        {saves && saves.length > 0 && (
          <>
            <h2 className="small">{t("picker.pick")}</h2>
            <ul className="save-list">
              {saves.map((s) => (
                <li key={s.name} className="save">
                  <div className="save-main">
                    <strong>{s.name}</strong>{" "}
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
                  <button type="button" onClick={() => void open(s.name)} aria-label={`${t("saves.switch")} ${s.name}`}>
                    {t("saves.switch")}
                  </button>
                </li>
              ))}
            </ul>
          </>
        )}
        <form
          className="stack"
          onSubmit={(e) => {
            e.preventDefault();
            void create();
          }}
        >
          <div className="underline-field">
            <label htmlFor={`${id}-new`}>{t("picker.new.name")}</label>
            <input id={`${id}-new`} value={fresh} placeholder={t("picker.new.default")} autoComplete="off" onChange={(e) => setFresh(e.target.value)} />
          </div>
          <p className="row end">
            <button type="submit">{t("picker.new.go")}</button>
          </p>
        </form>
        <form
          className="row wrap"
          onSubmit={(e) => {
            e.preventDefault();
            if (source.trim()) void importFile(null);
          }}
        >
          <input aria-label={t("saves.import.path")} placeholder={t("saves.import.path")} value={source} onChange={(e) => setSource(e.target.value)} />
          <input aria-label={t("saves.import.name")} placeholder={t("saves.import.name")} value={importName} onChange={(e) => setImportName(e.target.value)} />
          <button type="submit" className="ghost" disabled={!source.trim()}>
            {t("saves.import.go")}
          </button>
        </form>
        {error && <InlineError error={error} />}
        {status && <FolderNote text={t("picker.folder")} path={status.saves_dir} />}
      </div>
      {prompt && <PasswordDialog title={prompt.title} submit={prompt.submit} onClose={() => setPrompt(null)} />}
    </main>
  );
}

const WIRES: WireApi[] = ["chat", "responses", "messages"];

/** The last first-run step: one endpoint form (or just the key), skippable. */
export function ModelStep({ onDone }: { onDone: () => void }) {
  const t = useT();
  const api = useApi();
  const endpoints = useStore((s) => s.endpoints);
  const id = useId();
  const chat = endpoints?.roles?.chat;
  const keyless = chat ? endpoints?.profiles.find((p) => p.profile.name === chat.profile && !p.has_key) : undefined;
  const presets = endpoints?.presets ?? [];
  const [name, setName] = useState("main");
  const [url, setUrl] = useState("");
  const [wire, setWire] = useState<WireApi>("chat");
  const [model, setModel] = useState(chat?.model ?? "");
  const [host, setHost] = useState("");
  const [key, setKey] = useState("");
  const [error, setError] = useState<ApiError | Failure | null>(null);
  const [busy, setBusy] = useState(false);

  const preset = (pick: string) => {
    const p = presets.find((x) => x.name === pick);
    if (!p) return;
    setName(p.name);
    setUrl(p.base_url);
    setWire(p.wire_api);
  };

  const submit = async () => {
    setBusy(true);
    setError(null);
    const r = keyless
      ? await settle(api.commands.setKey(keyless.profile.name, key))
      : await settle(api.commands.setupEndpoint({ name, base_url: url, wire_api: wire, model, host_model: host.trim() || null, key: key.trim() || null }));
    setBusy(false);
    if (r.ok) onDone();
    else setError(r.error);
  };

  const field = error?.code === "invalid" ? error.field : null;
  const invalid = field ? (`model.invalid.${field}` as Key) : null;
  const known = ["name", "base_url", "model", "host_model", "key"];

  return (
    <main className="first-open setup">
      <form
        className="card"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <h1>{t("model.title")}</h1>
        {keyless ? <p>{t("model.key.only", { name: keyless.profile.name })}</p> : <p>{t("model.intro")}</p>}
        {!keyless && (
          <>
            {presets.length > 0 && (
              <div className="underline-field">
                <label htmlFor={`${id}-preset`}>{t("model.preset")}</label>
                <select id={`${id}-preset`} defaultValue="" onChange={(e) => preset(e.target.value)}>
                  <option value="">{t("model.preset.none")}</option>
                  {presets.map((p) => (
                    <option key={p.name} value={p.name}>
                      {p.name}
                    </option>
                  ))}
                </select>
              </div>
            )}
            <div className="underline-field">
              <label htmlFor={`${id}-name`}>{t("model.name")}</label>
              <input id={`${id}-name`} value={name} autoComplete="off" onChange={(e) => setName(e.target.value)} />
            </div>
            <div className="underline-field">
              <label htmlFor={`${id}-url`}>{t("model.url")}</label>
              <input id={`${id}-url`} value={url} placeholder="https://api.example.com/v1" autoComplete="off" spellCheck={false} onChange={(e) => setUrl(e.target.value)} />
            </div>
            <div className="underline-field">
              <label htmlFor={`${id}-wire`}>{t("model.wire")}</label>
              <select id={`${id}-wire`} value={wire} onChange={(e) => setWire(e.target.value as WireApi)}>
                {WIRES.map((w) => (
                  <option key={w} value={w}>
                    {t(`endpoints.wire.${w}`)}
                  </option>
                ))}
              </select>
            </div>
            <div className="underline-field">
              <label htmlFor={`${id}-model`}>{t("model.model")}</label>
              <input id={`${id}-model`} value={model} autoComplete="off" spellCheck={false} onChange={(e) => setModel(e.target.value)} />
            </div>
            <div className="underline-field">
              <label htmlFor={`${id}-host`}>{t("model.host")}</label>
              <input id={`${id}-host`} value={host} autoComplete="off" spellCheck={false} onChange={(e) => setHost(e.target.value)} />
            </div>
          </>
        )}
        <div className="underline-field">
          <label htmlFor={`${id}-key`}>{t("model.key")}</label>
          <input id={`${id}-key`} type="password" value={key} autoComplete="off" onChange={(e) => setKey(e.target.value)} />
        </div>
        {error && field && invalid && known.includes(field) ? (
          <p className="inline-error" role="alert">
            {t(invalid)}
          </p>
        ) : (
          error && <InlineError error={error as Failure} />
        )}
        <p className="row end">
          <button type="button" className="ghost" onClick={onDone}>
            {t("model.skip")}
          </button>
          <button type="submit" disabled={busy || (keyless ? !key.trim() : !url.trim() || !model.trim())}>
            {t("model.save")}
          </button>
        </p>
      </form>
    </main>
  );
}
