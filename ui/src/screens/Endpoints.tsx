import { useId, useState } from "react";
import { type EndpointsView, type ProfileView, type Role, type RoleKind } from "../api";
import { useT } from "../i18n";
import { useActions, useApi, useStore } from "../store";
import { InlineError } from "../ui/ErrorBar";
import { EndpointEditor, isLocal } from "./EndpointEditor";

const REASONING = ["low", "medium", "high"] as const;
const KINDS = ["chat", "host", "tts"] as const;

/** Settings, endpoints page: the roles on top, then the profiles they point at. */
export function EndpointsPage() {
  const t = useT();
  const view = useStore((s) => s.endpoints);
  const loadError = useStore((s) => s.loadErrors.endpoints);
  const actions = useActions();
  const [adding, setAdding] = useState(false);
  // A profile the add form has already written, while that form is still open.
  const [draft, setDraft] = useState<string | null>(null);
  if (!view) {
    return loadError ? (
      <InlineError error={loadError} onRetry={() => void actions.refreshEndpoints()} />
    ) : (
      <p className="muted" role="status">
        {t("app.loading")}
      </p>
    );
  }
  const listed = view.profiles.filter((p) => p.profile.name !== draft);
  const open = adding || view.profiles.length === 0;
  const close = () => {
    setAdding(false);
    setDraft(null);
  };

  return (
    <section className="endpoints" aria-labelledby="endpoints-h">
      <h3 id="endpoints-h">{t("endpoints.title")}</h3>
      {loadError && <InlineError error={loadError} onRetry={() => void actions.refreshEndpoints()} />}
      {listed.length > 0 && <Roles view={view} />}
      <section className="profiles-section" aria-labelledby="endpoints-list-h">
        <header className="row">
          <h4 id="endpoints-list-h">{t("endpoints.list")}</h4>
          <span className="spacer" />
          {!open && (
            <button type="button" className="ghost" onClick={() => setAdding(true)}>
              + {t("endpoints.add")}
            </button>
          )}
        </header>
        {open && (
          <div className="card add">
            {view.profiles.length === 0 && <p className="muted">{t("endpoints.empty")}</p>}
            <EndpointEditor view={view} focus={adding} onSaved={setDraft} onDone={close} onCancel={view.profiles.length > 0 ? close : undefined} />
          </div>
        )}
        <ul className="profiles">
          {listed.map((p) => (
            <ProfileCard key={p.profile.name} view={p} all={view} />
          ))}
        </ul>
      </section>
    </section>
  );
}

function ProfileCard({ view, all }: { view: ProfileView; all: EndpointsView }) {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const p = view.profile;
  const [editing, setEditing] = useState(false);
  const [asking, setAsking] = useState(false);
  const roles = all.roles;
  const held = KINDS.filter((k) => roles?.[k]?.profile === p.name);
  // The engine refuses to remove a profile any role (the maintainer's included) points at.
  const used = held.length > 0 || roles?.persona?.profile === p.name;
  const count = new Set((p.model ?? []).map((m) => m.id)).size;

  const remove = async () => {
    if ((await actions.attempt(api.commands.removeProfile(p.name))) === undefined) return;
    await actions.refreshEndpoints();
    await actions.refreshStatus();
  };

  return (
    <li className={`card profile${roles?.chat.profile === p.name ? " active" : ""}`}>
      <header className="row wrap">
        <h4>{p.name}</h4>
        <span className="tag">{t(`endpoints.wire.${p.wire_api}`)}</span>
        {held.map((k) => (
          <span key={k} className="tag on">
            ● {t(`endpoints.role.${k}`)}
          </span>
        ))}
      </header>
      {editing ? (
        <EndpointEditor view={all} profile={view} focus onDone={() => setEditing(false)} onCancel={() => setEditing(false)} />
      ) : (
        <>
          <p className="muted small url">{p.base_url}</p>
          <p className="small">
            {view.has_key ? (
              <span className="muted">{t("endpoints.key.stored")}</span>
            ) : isLocal(p.base_url) ? (
              <span className="muted">{t("endpoints.key.none_local")}</span>
            ) : (
              <span className="key-missing">{t("endpoints.key.missing")}</span>
            )}
            <span className="muted"> · {t("endpoints.models.count", { count })}</span>
          </p>
          {asking ? (
            <div role="group" aria-label={t("endpoints.remove.label", { name: p.name })}>
              <p>{t("endpoints.remove.ask", { name: p.name })}</p>
              <p className="row end">
                <button type="button" className="ghost" autoFocus onClick={() => setAsking(false)}>
                  {t("endpoints.cancel")}
                </button>
                <button type="button" className="danger" onClick={remove}>
                  {t("endpoints.remove")}
                </button>
              </p>
            </div>
          ) : (
            <p className="row end">
              {used && <span className="muted small spacer">{t("endpoints.remove.used")}</span>}
              {!used && (
                <button type="button" className="ghost danger" aria-label={t("endpoints.remove.label", { name: p.name })} onClick={() => setAsking(true)}>
                  {t("endpoints.remove")}
                </button>
              )}
              <button type="button" className="ghost" aria-label={t("endpoints.edit.label", { name: p.name })} onClick={() => setEditing(true)}>
                {t("endpoints.edit")}
              </button>
            </p>
          )}
        </>
      )}
    </li>
  );
}

const keyOf = (r: { profile: string; model: string } | null | undefined) => (r ? `${r.profile}\u0000${r.model}` : "");
const parse = (v: string) => {
  const [profile, model] = v.split("\u0000");
  return { profile, model };
};

/** One row per role the user may set; the maintainer-only persona role never shows. */
function Roles({ view }: { view: EndpointsView }) {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const id = useId();
  const roles = view.roles;
  const options = view.profiles.flatMap((p) => [...new Set((p.profile.model ?? []).map((m) => m.id))].map((m) => ({ profile: p.profile.name, model: m })));
  const withCurrent = (r: Role | null | undefined) => (r && !options.some((o) => keyOf(o) === keyOf(r)) ? [r, ...options] : options);

  const setRole = async (kind: RoleKind, role: Role | null) => {
    if ((await actions.attempt(api.commands.setRole(kind, role))) === undefined) return;
    await actions.refreshEndpoints();
    await actions.refreshStatus();
  };

  const rows = [
    { kind: "chat" as const, role: roles?.chat, none: roles ? null : t("endpoints.role.none"), reasoning: true },
    { kind: "host" as const, role: roles?.host, none: t("endpoints.role.host.same"), reasoning: true },
    { kind: "tts" as const, role: roles?.tts, none: t("endpoints.role.off"), reasoning: false },
  ];

  return (
    <section className="roles" aria-labelledby={`${id}-h`}>
      <h4 id={`${id}-h`}>{t("endpoints.roles")}</h4>
      <table className="roles-table" aria-describedby={`${id}-hint`}>
        <thead>
          <tr>
            <th scope="col">{t("endpoints.role")}</th>
            <th scope="col">{t("endpoints.model")}</th>
            <th scope="col">{t("endpoints.reasoning")}</th>
          </tr>
        </thead>
        <tbody>
          {rows.map(({ kind, role, none, reasoning }) => {
            const label = t(`endpoints.role.${kind}`);
            const current = roles?.[kind] ?? null;
            // Without a chat role nothing else can be set.
            const off = !roles && kind !== "chat";
            return (
              <tr key={kind}>
                <th scope="row">
                  <label htmlFor={`${id}-${kind}`}>{label}</label>
                </th>
                <td>
                  <select
                    id={`${id}-${kind}`}
                    disabled={off}
                    value={keyOf(role)}
                    onChange={(e) => {
                      const v = e.target.value;
                      if (!v && kind === "chat") return;
                      void setRole(kind, v ? { ...parse(v), reasoning: reasoning ? (current?.reasoning ?? null) : undefined } : null);
                    }}
                  >
                    {none !== null && <option value="">{none}</option>}
                    {withCurrent(role).map((o) => (
                      <option key={keyOf(o)} value={keyOf(o)}>
                        {o.profile} · {o.model}
                      </option>
                    ))}
                  </select>
                </td>
                <td>
                  {reasoning ? (
                    <select
                      aria-label={t("endpoints.reasoning.of", { role: label })}
                      disabled={!current}
                      value={current?.reasoning ?? ""}
                      onChange={(e) => current && void setRole(kind, { ...current, reasoning: e.target.value || null })}
                    >
                      <option value="">{t("endpoints.reasoning.default")}</option>
                      {REASONING.map((r) => (
                        <option key={r} value={r}>
                          {t(`endpoints.reasoning.${r}`)}
                        </option>
                      ))}
                    </select>
                  ) : (
                    <span className="muted" aria-hidden="true">
                      –
                    </span>
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      {options.length === 0 && <p className="muted small">{t("endpoints.roles.empty")}</p>}
      <p id={`${id}-hint`} className="muted small">
        {t("endpoints.reasoning.hint")}
      </p>
    </section>
  );
}
