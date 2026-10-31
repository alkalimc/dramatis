import { useEffect, useId, useState } from "react";
import { settle, type CorpusState, type Failure } from "../api";
import { useT } from "../i18n";
import { useActions, useApi, useStore } from "../store";
import { readAvatar } from "../lib/image";
import { UserAvatar } from "../ui/Avatar";
import { Credit } from "../ui/Credit";
import { Icon } from "../ui/Icon";
import { ErrorBar, InlineError } from "../ui/ErrorBar";
import { PictureButtons } from "./Profile";
import { FolderNote, SavesPage } from "./Saves";

/** The first-run screens: the corpus, the save, the model. Each says what it needs and checks the answer. */

const reload = () => window.location.reload();

/** Top left of a first-run card: one step back. */
export function StepBack({ onBack }: { onBack: () => void }) {
  const t = useT();
  return (
    <button type="button" className="icon step-back" aria-label={t("chatlist.back")} title={t("chatlist.back")} onClick={onBack}>
      <Icon name="back" size={22} />
    </button>
  );
}

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
        <Credit />
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

/** No save is open: the saves page (pick, create, rename, copy, delete, export, import). Opening reloads the window onto it. */
export function SavePicker({ onSwitched = reload }: { onSwitched?: () => void }) {
  const t = useT();
  const status = useStore((s) => s.status);
  const [count, setCount] = useState<number | null>(null);
  const first = count === 0;
  const missing = status?.missing_save;

  return (
    <main className="first-open setup picker">
      <div className="card">
        <h1>{first ? t("picker.first") : t("picker.title")}</h1>
        <Credit />
        {missing && <p role="alert">{t("picker.missing", { name: missing })}</p>}
        {first && <p>{t("picker.first.hint")}</p>}
        <SavesPage picker onSwitched={onSwitched} onCount={setCount} />
      </div>
      <ErrorBar />
    </main>
  );
}

/** Asked once, when the home has no answer yet: whether to check for a newer version by itself. */
export function UpdateStep({ onBack }: { onBack?: () => void }) {
  const t = useT();
  const actions = useActions();
  const [busy, setBusy] = useState(false);
  const answer = async (on: boolean) => {
    setBusy(true);
    await actions.setAutoUpdate(on);
    setBusy(false);
  };
  return (
    <main className="first-open setup">
      <div className="card">
        {onBack && <StepBack onBack={onBack} />}
        <h1>{t("update.ask.title")}</h1>
        <Credit />
        <p>{t("update.ask.hint")}</p>
        <p className="row end">
          <button type="button" className="ghost" disabled={busy} onClick={() => void answer(false)}>
            {t("update.ask.no")}
          </button>
          <button type="button" disabled={busy} onClick={() => void answer(true)}>
            {t("update.ask.yes")}
          </button>
        </p>
      </div>
    </main>
  );
}

/** After the name on a first run: a picture to upload, or the default portrait. Skippable. */
export function AvatarStep({ onDone, onBack }: { onDone: () => void; onBack?: () => void }) {
  const t = useT();
  const actions = useActions();
  const avatar = useStore((s) => s.userAvatar);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    void actions.refreshUserAvatar();
  }, [actions]);
  return (
    <main className="first-open setup">
      <div className="card">
        {onBack && <StepBack onBack={onBack} />}
        <h1>{t("first.avatar.title")}</h1>
        <Credit />
        <p>{t("first.avatar.hint")}</p>
        <p className="avatar-pick">
          <UserAvatar size="xl" />
        </p>
        <PictureButtons
          clearable={false}
          onClear={() => {}}
          onPick={async (f) => {
            setBusy(true);
            await actions.setUserAvatar(await readAvatar(f));
            setBusy(false);
          }}
        />
        <p className="row end">
          <button type="button" disabled={busy} onClick={onDone}>
            {t(avatar && !busy ? "first.avatar.next" : "first.avatar.skip")}
          </button>
        </p>
      </div>
    </main>
  );
}
