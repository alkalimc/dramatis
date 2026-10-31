import { useEffect, useId, useRef, useState } from "react";
import { settle, type EndpointsView, type Failure, type Model, type Profile, type ProfileView, type WireApi } from "../api";
import { useT } from "../i18n";
import { useActions, useApi } from "../store";
import { errorText, InlineError } from "../ui/ErrorBar";
import { ModelList } from "./EndpointModels";

export const WIRES: WireApi[] = ["chat", "responses", "messages"];
// What the engine accepts as an address, so the form refuses it before asking.
const ADDRESS = /^https?:\/\/\S+$/;

/** A server on this computer: it needs no key. */
export function isLocal(url: string): boolean {
  try {
    const host = new URL(url.trim()).hostname;
    return host === "127.0.0.1" || host === "localhost" || host === "[::1]";
  } catch {
    return false;
  }
}

/** A profile that has no key and is not on this computer: calls to it would be refused. */
export const lacksKey = (p: ProfileView) => !p.has_key && !isLocal(p.profile.base_url);

/** Whether the chat role still needs setting: none yet, or its profile has no key it needs. */
export function needsSetup(view: EndpointsView): boolean {
  const chat = view.roles?.chat;
  return !chat || view.profiles.some((p) => p.profile.name === chat.profile && lacksKey(p));
}

/** The wire choices, each with a hint on what it speaks. */
export function WireOptions() {
  const t = useT();
  return (
    <>
      {WIRES.map((w) => (
        <option key={w} value={w}>
          {t("endpoints.wire.option", { wire: t(`endpoints.wire.${w}`), hint: t(`endpoints.wire.${w}.hint`) })}
        </option>
      ))}
    </>
  );
}

const free = (base: string, taken: Set<string>) => {
  if (!taken.has(base)) return base;
  for (let n = 2; ; n++) if (!taken.has(`${base}-${n}`)) return `${base}-${n}`;
};

const clean = (models: Model[]): Model[] => models.filter((m) => m.id.trim()).map((m) => ({ ...m, id: m.id.trim() }));

type Problem = "name" | "url" | "key" | "pick" | "models";

/**
 * The one editor for a profile, first run and settings alike: where it is (a preset or an
 * address), the key, a check that lists its models, and the model the chat role uses.
 * New: nothing is asked of the engine until "Check connection". Existing: the same fields,
 * filled, plus the model list with its limits and prices. `guided` is the first-run
 * screen: it leaves the store to the caller, which reloads everything when it is done.
 */
export function EndpointEditor({
  view,
  profile,
  guided = false,
  focus = false,
  onSaved,
  onDone,
  onCancel,
}: {
  view: EndpointsView;
  /** The profile being edited; absent: a new one. */
  profile?: ProfileView;
  guided?: boolean;
  /** Put the keyboard on the first field that needs it when the editor opens. */
  focus?: boolean;
  /** The profile now exists under this name, though the editor is still open. */
  onSaved?: (name: string) => void;
  onDone: () => void;
  onCancel?: () => void;
}) {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const id = useId();
  const chat = view.roles?.chat ?? null;
  const editing = profile !== undefined;
  const first = profile?.profile ?? null;
  const taken = new Set(view.profiles.map((p) => p.profile.name));

  const [preset, setPreset] = useState<number | null>(null);
  const [name, setName] = useState(first?.name ?? free("main", taken));
  const [url, setUrl] = useState(first?.base_url ?? "");
  const [wire, setWire] = useState<WireApi>(first?.wire_api ?? "chat");
  const [key, setKey] = useState("");
  const [replacing, setReplacing] = useState(false);
  const [stored, setStored] = useState(false);
  const [models, setModels] = useState<Model[]>(first?.model ?? []);
  const [saved, setSaved] = useState<string | null>(first?.name ?? null);
  const [checked, setChecked] = useState(editing);
  const [pick, setPick] = useState(() => (chat && first && chat.profile === first.name ? chat.model : chat ? "" : (first?.model?.[0]?.id ?? "")));
  const [list, setList] = useState<{ error: Failure | null; found: number | null }>({ error: null, found: null });
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<Failure | null>(null);
  const [touched, setTouched] = useState(false);
  const [focusModel, setFocusModel] = useState(false);

  const chipsRef = useRef<HTMLFieldSetElement>(null);
  const nameRef = useRef<HTMLInputElement>(null);
  const urlRef = useRef<HTMLInputElement>(null);
  const keyRef = useRef<HTMLInputElement>(null);
  const modelRef = useRef<HTMLElement | null>(null);

  const local = isLocal(url);
  const hasKey = stored || Boolean(view.profiles.find((p) => p.profile.name === saved)?.has_key);
  const showKey = !hasKey || replacing;
  const ids = [...new Set(clean(models).map((m) => m.id))];
  const own = Boolean(chat && saved && chat.profile === saved);
  const keep = chat !== null && !own;
  const wanted = pick.trim();
  const willChat = wanted !== "" && !(own && chat && wanted === chat.model);
  const duplicate = clean(models).length !== ids.length;

  // The first thing that is wrong, in the order the form is read.
  const problem: Problem | null =
    !name.trim() || (!editing && name.trim() !== saved && taken.has(name.trim()))
      ? "name"
      : !ADDRESS.test(url.trim())
        ? "url"
        : !local && !hasKey && !key.trim() && (!checked || willChat || own || chat === null)
          ? "key"
          : checked && (/\s/.test(wanted) || (chat === null && !wanted))
            ? "pick"
            : duplicate
              ? "models"
              : null;
  const shown = touched ? problem : null;

  useEffect(() => {
    if (!focus) return;
    const target = editing ? (!hasKey && !local ? keyRef.current : urlRef.current) : chipsRef.current?.querySelector("input");
    target?.focus();
    // Only when the editor opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => {
    if (!focusModel) return;
    modelRef.current?.focus();
    setFocusModel(false);
  }, [focusModel]);

  const choose = async (i: number) => {
    // A draft already written belongs to the address before, and so does its key: drop both.
    if (saved !== null && !editing) {
      const r = await settle(api.commands.removeProfile(saved));
      if (!r.ok) return setFailure(r.error);
      setSaved(null);
      setStored(false);
      setReplacing(false);
      if (!guided) await actions.refreshEndpoints();
    }
    const p = i >= 0 ? view.presets[i] : null;
    setPreset(i);
    setName(free(p?.name ?? "custom", new Set([...taken].filter((n) => n !== saved))));
    setUrl(p?.base_url ?? "");
    setWire(p?.wire_api ?? "chat");
    setKey("");
    setFailure(null);
    // What was listed belonged to the address before.
    setModels([]);
    setPick("");
    setChecked(false);
    setList({ error: null, found: null });
    if (!p) urlRef.current?.focus();
  };

  const flag = (p: Problem) => {
    setTouched(true);
    const target = { name: nameRef.current, url: urlRef.current, key: keyRef.current, pick: modelRef.current, models: null }[p];
    target?.focus();
  };

  const profileOf = (next: Model[], as = name.trim()): Profile => ({ ...(first ?? {}), name: as, base_url: url.trim(), wire_api: wire, model: next });

  /** Write the profile and a typed key; the name it now has, or null with the reason shown. */
  const persist = async (next: Model[]): Promise<string | null> => {
    const p = profileOf(next);
    const r = saved === null ? await settle(api.commands.addProfile(p)) : await settle(api.commands.updateProfile(saved, p));
    if (!r.ok) {
      setFailure(r.error);
      return null;
    }
    setSaved(p.name);
    onSaved?.(p.name);
    const secret = key.trim();
    if (secret) {
      const k = await settle(api.commands.setKey(p.name, secret));
      if (!k.ok) {
        setFailure(k.error);
        return null;
      }
      // Once stored, the key is not held here any more.
      setKey("");
      setStored(true);
      setReplacing(false);
    }
    return p.name;
  };

  const sync = async () => {
    // The first-run screen is shown for as long as the store says nothing is set up.
    if (guided) return;
    await actions.refreshEndpoints();
    await actions.refreshStatus();
  };

  const check = async () => {
    if (problem && problem !== "pick") return flag(problem);
    setBusy(true);
    setFailure(null);
    setList({ error: null, found: null });
    const n = await persist(clean(models));
    if (n === null) return setBusy(false);
    const r = await settle(api.commands.fetchModels(n));
    if (!r.ok) {
      setList({ error: r.error, found: null });
      setChecked(true);
      setBusy(false);
      if (!guided) await actions.refreshEndpoints();
      return;
    }
    // Remember what the endpoint offers, so the list survives a restart.
    const known = new Set(ids);
    const added = r.data.filter((m) => !known.has(m));
    if (added.length) {
      const next = [...clean(models), ...added.map((m) => ({ id: m }))];
      setModels(next);
      const u = await settle(api.commands.updateProfile(n, profileOf(next, n)));
      if (!u.ok) setFailure(u.error);
    }
    setList({ error: null, found: r.data.length });
    if (!pick && chat === null && r.data[0]) setPick(r.data[0]);
    setChecked(true);
    setBusy(false);
    setFocusModel(true);
    if (!guided) await actions.refreshEndpoints();
  };

  const commit = async () => {
    if (problem) return flag(problem);
    let next = clean(models);
    if (wanted && !next.some((m) => m.id === wanted)) next = [...next, { id: wanted }];
    setBusy(true);
    setFailure(null);
    setModels(next);
    const n = await persist(next);
    if (n === null) return setBusy(false);
    if (willChat) {
      const r = await settle(api.commands.setRole("chat", { profile: n, model: wanted, reasoning: chat?.reasoning ?? null }));
      if (!r.ok) {
        setFailure(r.error);
        return setBusy(false);
      }
    }
    await sync();
    setBusy(false);
    onDone();
  };

  const fail = list.error;
  const authish = fail !== null && "detail" in fail && /\b(401|403)\b/.test(fail.detail);
  const shownFail = fail ? errorText(fail, t, {}) : null;
  const picker = ids.length > 0;
  const cancelLabel = guided ? t("endpoints.skip") : t("endpoints.cancel");

  return (
    <form
      className="endpoint-form"
      noValidate
      onSubmit={(e) => {
        e.preventDefault();
        if (!busy) void (checked ? commit() : check());
      }}
    >
      {!editing && (
        <fieldset className="presets" ref={chipsRef}>
          <legend>{t("endpoints.preset")}</legend>
          <div className="chips">
            {view.presets.map((p, i) => (
              <label key={p.name} className={`chip${preset === i ? " on" : ""}`}>
                <input type="radio" name={`${id}-preset`} className="sr-only" checked={preset === i} onChange={() => void choose(i)} />
                {p.name}
              </label>
            ))}
            <label className={`chip${preset === -1 ? " on" : ""}`}>
              <input type="radio" name={`${id}-preset`} className="sr-only" checked={preset === -1} onChange={() => void choose(-1)} />
              {t("endpoints.preset.custom")}
            </label>
          </div>
        </fieldset>
      )}

      <div className="field">
        <label htmlFor={`${id}-url`}>{t("endpoints.preset.url")}</label>
        <input
          id={`${id}-url`}
          ref={urlRef}
          type="url"
          value={url}
          placeholder="https://api.example.com/v1"
          autoComplete="off"
          spellCheck={false}
          aria-invalid={shown === "url"}
          aria-describedby={shown === "url" ? `${id}-url-e` : undefined}
          onChange={(e) => setUrl(e.target.value)}
        />
        {shown === "url" && (
          <p id={`${id}-url-e`} className="inline-error" role="alert">
            {t("error.invalid.base_url")}
          </p>
        )}
      </div>

      {editing ? (
        <div className="field">
          <label htmlFor={`${id}-wire`}>{t("endpoints.preset.wire")}</label>
          <select id={`${id}-wire`} value={wire} onChange={(e) => setWire(e.target.value as WireApi)}>
            <WireOptions />
          </select>
        </div>
      ) : (
        <details className="more" open={preset === -1 || shown === "name" || undefined}>
          <summary>{t("endpoints.details")}</summary>
          <div className="field">
            <label htmlFor={`${id}-name`}>{t("endpoints.preset.name")}</label>
            <input
              id={`${id}-name`}
              ref={nameRef}
              value={name}
              autoComplete="off"
              aria-invalid={shown === "name"}
              aria-describedby={shown === "name" ? `${id}-name-e` : undefined}
              onChange={(e) => setName(e.target.value)}
            />
            {shown === "name" && (
              <p id={`${id}-name-e`} className="inline-error" role="alert">
                {t("endpoints.invalid.name")}
              </p>
            )}
          </div>
          <div className="field">
            <label htmlFor={`${id}-wire`}>{t("endpoints.preset.wire")}</label>
            <select id={`${id}-wire`} value={wire} onChange={(e) => setWire(e.target.value as WireApi)}>
              <WireOptions />
            </select>
          </div>
        </details>
      )}

      <div className="field">
        {showKey ? <label htmlFor={`${id}-key`}>{t("endpoints.key")}</label> : <span>{t("endpoints.key")}</span>}
        <div className="row field-row">
          {showKey ? (
            <input
              id={`${id}-key`}
              ref={keyRef}
              type="password"
              value={key}
              autoComplete="off"
              spellCheck={false}
              aria-invalid={shown === "key"}
              aria-describedby={`${id}-key-h${shown === "key" ? ` ${id}-key-e` : ""}`}
              onChange={(e) => setKey(e.target.value)}
            />
          ) : (
            <span className="key-stored">{t("endpoints.key.stored")}</span>
          )}
          {!showKey && (
            <button type="button" className="ghost" onClick={() => setReplacing(true)}>
              {t("endpoints.key.replace")}
            </button>
          )}
          {showKey && replacing && (
            <button
              type="button"
              className="ghost"
              onClick={() => {
                setReplacing(false);
                setKey("");
              }}
            >
              {t("endpoints.cancel")}
            </button>
          )}
        </div>
        <p id={`${id}-key-h`} className="muted small">
          {t(local ? "endpoints.key.local" : "endpoints.key.hint")}
        </p>
        {shown === "key" && (
          <p id={`${id}-key-e`} className="inline-error" role="alert">
            {t("endpoints.invalid.key")}
          </p>
        )}
      </div>

      {checked && (
        <div className="field">
          <label htmlFor={`${id}-model`}>{t("endpoints.model.chat")}</label>
          <div className="row field-row">
            {picker ? (
              <select
                id={`${id}-model`}
                ref={(el) => {
                  modelRef.current = el;
                }}
                value={pick}
                aria-invalid={shown === "pick"}
                onChange={(e) => setPick(e.target.value)}
              >
                {keep && <option value="">{t("endpoints.model.keep", { profile: chat!.profile, model: chat!.model })}</option>}
                {pick && !ids.includes(pick) && <option value={pick}>{pick}</option>}
                {ids.map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </select>
            ) : (
              <input
                id={`${id}-model`}
                ref={(el) => {
                  modelRef.current = el;
                }}
                value={pick}
                autoComplete="off"
                spellCheck={false}
                aria-invalid={shown === "pick"}
                aria-describedby={keep ? `${id}-model-h` : undefined}
                onChange={(e) => setPick(e.target.value)}
              />
            )}
            <button type="button" className="ghost" disabled={busy} aria-busy={busy} onClick={() => void check()}>
              {busy ? t("endpoints.fetching") : t("endpoints.fetch")}
            </button>
          </div>
          {keep && !picker && (
            <p id={`${id}-model-h`} className="muted small">
              {t("endpoints.model.keep.hint", { profile: chat!.profile, model: chat!.model })}
            </p>
          )}
          {shown === "pick" && (
            <p className="inline-error" role="alert">
              {/\s/.test(wanted) ? t("error.invalid.model") : t("endpoints.invalid.pick")}
            </p>
          )}
          {list.found !== null && (
            <p className="muted small" role="status">
              {list.found === 0 ? t("endpoints.fetch.none") : t("endpoints.fetch.found", { count: list.found })}
            </p>
          )}
        </div>
      )}
      {shownFail && (
        <div className="fetch-error" role="alert">
          <p>{t(picker ? "endpoints.fetch.failed.known" : "endpoints.fetch.failed")}</p>
          <p className="muted small">{shownFail.text}</p>
          <p className="muted small">{t(authish ? "endpoints.hint.key" : "endpoints.hint.address")}</p>
          {shownFail.detail && <code className="raw">{shownFail.detail}</code>}
        </div>
      )}

      {editing && (
        <details className="more" open={shown === "models" || undefined}>
          <summary>{t("endpoints.models")}</summary>
          <ModelList models={models} wire={wire} duplicate={shown === "models"} onChange={setModels} />
        </details>
      )}

      {failure && <InlineError error={failure} />}
      <p className="row end">
        {onCancel && (
          <button type="button" className="ghost" onClick={onCancel}>
            {cancelLabel}
          </button>
        )}
        <button type="submit" disabled={busy} aria-busy={busy}>
          {busy ? t("endpoints.checking") : checked ? t(guided ? "endpoints.save.continue" : "endpoints.save") : t("endpoints.check")}
        </button>
      </p>
    </form>
  );
}
