import { useId, useState } from "react";
import type { ApiError, EndpointsView, Profile, ProfileView, Role, WireApi } from "../api";
import { useT } from "../i18n";
import { useActions, useApi, useStore } from "../store";

const REASONING = ["low", "medium", "high"] as const;

type Fetched = Record<string, { models?: string[]; error?: ApiError }>;

function knownModels(p: Profile, fetched: Fetched): string[] {
  return [...new Set([...(fetched[p.name]?.models ?? []), ...(p.model ?? []).map((m) => m.id)])];
}

/** System drawer, endpoints tab: profiles as cards, roles pointing at them. */
export function EndpointsPage() {
  const t = useT();
  const view = useStore((s) => s.endpoints);
  const [fetched, setFetched] = useState<Fetched>({});
  const [adding, setAdding] = useState(false);
  if (!view) return <p className="muted">{t("app.loading")}</p>;
  const chat = view.roles?.chat ?? null;

  return (
    <section className="endpoints" aria-labelledby="endpoints-h">
      <header className="drawer-head">
        <h3 id="endpoints-h">{t("endpoints.title")}</h3>
        {!adding && (
          <button type="button" onClick={() => setAdding(true)}>
            + {t("endpoints.add")}
          </button>
        )}
      </header>
      {adding && <AddProfile view={view} onDone={() => setAdding(false)} />}
      {view.profiles.length === 0 && !adding && <p className="muted">{t("endpoints.empty")}</p>}
      <ul className="profiles">
        {view.profiles.map((p) => (
          <ProfileCard
            key={p.profile.name}
            view={p}
            active={chat?.profile === p.profile.name}
            role={chat}
            fetched={fetched}
            onFetched={(name, r) => setFetched((f) => ({ ...f, [name]: r }))}
          />
        ))}
      </ul>
      {view.profiles.length > 0 && <Roles view={view} fetched={fetched} />}
    </section>
  );
}

function ProfileCard({
  view,
  active,
  role,
  fetched,
  onFetched,
}: {
  view: ProfileView;
  active: boolean;
  role: Role | null;
  fetched: Fetched;
  onFetched: (name: string, r: Fetched[string]) => void;
}) {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const id = useId();
  const p = view.profile;
  const models = knownModels(p, fetched);
  const failed = fetched[p.name]?.error;
  const [model, setModel] = useState(active && role ? role.model : (models[0] ?? ""));
  const [busy, setBusy] = useState(false);

  const fetchModels = async () => {
    setBusy(true);
    const r = await api.commands.fetchModels(p.name);
    setBusy(false);
    if (r.status === "error") return onFetched(p.name, { error: r.error });
    onFetched(p.name, { models: r.data });
    if (!model && r.data[0]) setModel(r.data[0]);
    // Remember what the endpoint offers, so the list survives a restart.
    const known = new Set((p.model ?? []).map((m) => m.id));
    const added = r.data.filter((m) => !known.has(m)).map((m) => ({ id: m }));
    if (added.length) {
      await actions.attempt(api.commands.updateProfile(p.name, { ...p, model: [...(p.model ?? []), ...added] }));
      await actions.refreshEndpoints();
    }
  };

  const useModel = async (next: string) => {
    setModel(next);
    // On the card in use, picking a model is the switch; the next call uses it.
    if (active && role && next) {
      await actions.attempt(api.commands.setRole("chat", { ...role, model: next }));
      await actions.refreshEndpoints();
    }
  };

  const switchTo = async () => {
    if (!model) return;
    const ok = await actions.attempt(api.commands.setRole("chat", { profile: p.name, model, reasoning: role?.reasoning ?? null }));
    if (ok === undefined) return;
    await actions.refreshEndpoints();
    await actions.refreshStatus();
  };

  const remove = async () => {
    if ((await actions.attempt(api.commands.removeProfile(p.name))) === undefined) return;
    await actions.refreshEndpoints();
    await actions.refreshStatus();
  };

  return (
    <li className={`card profile${active ? " active" : ""}`}>
      <header className="row">
        <h4>{p.name}</h4>
        <span className="tag">{p.wire_api}</span>
        <span className="spacer" />
        {active ? (
          <span className="tag on">● {t("endpoints.in_use")}</span>
        ) : (
          <button type="button" className="ghost" disabled={!model} title={model ? undefined : t("endpoints.switch.need_model")} onClick={switchTo}>
            {t("endpoints.switch")}
          </button>
        )}
      </header>
      <p className="muted small url">{p.base_url}</p>
      <div className="row wrap">
        {failed || models.length === 0 ? (
          <>
            <label htmlFor={id}>{t("endpoints.model.manual")}</label>
            <input id={id} value={model} onChange={(e) => setModel(e.target.value)} onBlur={() => active && void useModel(model.trim())} />
          </>
        ) : (
          <>
            <label htmlFor={id}>{t("endpoints.model")}</label>
            <select id={id} value={model} onChange={(e) => void useModel(e.target.value)}>
              {!models.includes(model) && model && <option value={model}>{model}</option>}
              {models.map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </select>
          </>
        )}
        <button type="button" className="ghost" disabled={busy} onClick={fetchModels}>
          {busy ? t("endpoints.fetching") : t("endpoints.fetch")}
        </button>
      </div>
      {failed && (
        <div className="fetch-error" role="alert">
          <p>{t("endpoints.fetch.failed")}</p>
          {"detail" in failed && <pre className="raw">{failed.detail}</pre>}
        </div>
      )}
      <KeyField profile={p.name} hasKey={view.has_key} />
      {!active && (
        <p className="row end">
          <button type="button" className="link danger" onClick={remove}>
            {t("endpoints.remove")}
          </button>
        </p>
      )}
    </li>
  );
}

/** Write-only: once stored, the key is never shown or read back. */
function KeyField({ profile, hasKey }: { profile: string; hasKey: boolean }) {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const id = useId();
  const [key, setKey] = useState("");
  if (hasKey) return <p className="key-stored">{t("endpoints.key.stored")}</p>;
  return (
    <form
      className="row"
      onSubmit={async (e) => {
        e.preventDefault();
        if (!key.trim()) return;
        const ok = await actions.attempt(api.commands.setKey(profile, key.trim()));
        setKey("");
        if (ok !== undefined) await actions.refreshEndpoints();
      }}
    >
      <label htmlFor={id}>{t("endpoints.key")}</label>
      <input id={id} type="password" autoComplete="off" spellCheck={false} value={key} onChange={(e) => setKey(e.target.value)} placeholder={t("endpoints.key.missing")} />
      <button type="submit" disabled={!key.trim()}>
        {t("endpoints.key.save")}
      </button>
    </form>
  );
}

function AddProfile({ view, onDone }: { view: EndpointsView; onDone: () => void }) {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const id = useId();
  const [preset, setPreset] = useState(0);
  const base = view.presets[preset];
  const taken = new Set(view.profiles.map((p) => p.profile.name));
  const [name, setName] = useState(base?.name ?? "");
  const [url, setUrl] = useState(base?.base_url ?? "");
  const [wire, setWire] = useState<WireApi>(base?.wire_api ?? "chat");
  const [added, setAdded] = useState<string | null>(null);

  const choose = (i: number) => {
    const p = view.presets[i];
    setPreset(i);
    setName(taken.has(p.name) ? `${p.name}-2` : p.name);
    setUrl(p.base_url);
    setWire(p.wire_api);
  };

  if (added) {
    const hasKey = view.profiles.find((p) => p.profile.name === added)?.has_key ?? false;
    return (
      <div className="card add">
        <h4>{added}</h4>
        <KeyField profile={added} hasKey={hasKey} />
        <p className="row end">
          <button type="button" onClick={onDone}>
            {t("endpoints.preset.done")}
          </button>
        </p>
      </div>
    );
  }

  return (
    <form
      className="card add"
      onSubmit={async (e) => {
        e.preventDefault();
        const profile: Profile = { name: name.trim(), base_url: url.trim(), wire_api: wire, model: [] };
        if ((await actions.attempt(api.commands.addProfile(profile))) === undefined) return;
        await actions.refreshEndpoints();
        setAdded(profile.name);
      }}
    >
      <fieldset>
        <legend>{t("endpoints.preset")}</legend>
        <div className="chips">
          {view.presets.map((p, i) => (
            <label key={p.name} className={`chip${preset === i ? " on" : ""}`}>
              <input type="radio" name={`${id}-preset`} className="sr-only" checked={preset === i} onChange={() => choose(i)} />
              {p.name}
            </label>
          ))}
        </div>
      </fieldset>
      <label htmlFor={`${id}-n`}>{t("endpoints.preset.name")}</label>
      <input id={`${id}-n`} value={name} onChange={(e) => setName(e.target.value)} required />
      <label htmlFor={`${id}-u`}>{t("endpoints.preset.url")}</label>
      <input id={`${id}-u`} type="url" value={url} onChange={(e) => setUrl(e.target.value)} required />
      <label htmlFor={`${id}-w`}>{t("endpoints.preset.wire")}</label>
      <select id={`${id}-w`} value={wire} onChange={(e) => setWire(e.target.value as WireApi)}>
        <option value="chat">chat</option>
        <option value="responses">responses</option>
        <option value="messages">messages</option>
      </select>
      <p className="row end">
        <button type="button" className="ghost" onClick={onDone}>
          {t("endpoints.preset.cancel")}
        </button>
        <button type="submit" disabled={!name.trim() || !url.trim() || taken.has(name.trim())}>
          {t("endpoints.preset.next")}
        </button>
      </p>
    </form>
  );
}

/** One row per role the user may set; the maintainer-only persona role never shows. */
function Roles({ view, fetched }: { view: EndpointsView; fetched: Fetched }) {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const id = useId();
  const roles = view.roles;
  const options = view.profiles.flatMap((p) => knownModels(p.profile, fetched).map((m) => ({ profile: p.profile.name, model: m })));
  const key = (r: { profile: string; model: string } | null | undefined) => (r ? `${r.profile}\u0000${r.model}` : "");
  const parse = (v: string) => {
    const [profile, model] = v.split("\u0000");
    return { profile, model };
  };

  const setRole = async (kind: "chat" | "tts", role: Role | null) => {
    if ((await actions.attempt(api.commands.setRole(kind, role))) === undefined) return;
    await actions.refreshEndpoints();
    await actions.refreshStatus();
  };

  const withCurrent = (r: Role | null | undefined) => (r && !options.some((o) => key(o) === key(r)) ? [r, ...options] : options);

  return (
    <section className="roles" aria-labelledby={`${id}-h`}>
      <h4 id={`${id}-h`}>{t("endpoints.roles")}</h4>
      <div className="role-row">
        <label htmlFor={`${id}-chat`}>{t("endpoints.role.chat")}</label>
        <select
          id={`${id}-chat`}
          value={key(roles?.chat)}
          onChange={(e) => e.target.value && void setRole("chat", { ...parse(e.target.value), reasoning: roles?.chat.reasoning ?? null })}
        >
          {!roles && <option value="">{t("endpoints.role.none")}</option>}
          {withCurrent(roles?.chat).map((o) => (
            <option key={key(o)} value={key(o)}>
              {o.profile} · {o.model}
            </option>
          ))}
        </select>
        <label htmlFor={`${id}-reason`}>{t("endpoints.reasoning")}</label>
        <select
          id={`${id}-reason`}
          disabled={!roles}
          value={roles?.chat.reasoning ?? ""}
          aria-describedby={`${id}-reason-h`}
          onChange={(e) => roles && void setRole("chat", { ...roles.chat, reasoning: e.target.value || null })}
        >
          <option value="">{t("endpoints.reasoning.default")}</option>
          {REASONING.map((r) => (
            <option key={r} value={r}>
              {t(`endpoints.reasoning.${r}`)}
            </option>
          ))}
        </select>
      </div>
      <p id={`${id}-reason-h`} className="muted small">
        {t("endpoints.reasoning.hint")}
      </p>
      <div className="role-row">
        <label htmlFor={`${id}-tts`}>{t("endpoints.role.tts")}</label>
        <select
          id={`${id}-tts`}
          disabled={!roles}
          value={key(roles?.tts)}
          onChange={(e) => void setRole("tts", e.target.value ? parse(e.target.value) : null)}
        >
          <option value="">{t("endpoints.role.off")}</option>
          {withCurrent(roles?.tts).map((o) => (
            <option key={key(o)} value={key(o)}>
              {o.profile} · {o.model}
            </option>
          ))}
        </select>
      </div>
    </section>
  );
}
