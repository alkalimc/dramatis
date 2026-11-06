import { useId, useState } from "react";
import { type EndpointsView, type ProfileView, type Role, type RoleKind } from "../api";
import { useT } from "../i18n";
import { useActions, useApi, useDispatch, useStore } from "../store";
import { InlineError } from "../ui/ErrorBar";
import { Body, Field, Note, Row, Section } from "../ui/SettingsKit";
import { EndpointEditor, isLocal } from "./EndpointEditor";
import { EndpointTester } from "./EndpointTest";

const REASONING = ["low", "medium", "high"] as const;
const KINDS = ["chat", "host", "tts"] as const;

/** Settings, endpoints page: the roles on top, then the profiles they point at. */
export function EndpointsPage() {
  const t = useT();
  const view = useStore((s) => s.endpoints);
  const loadError = useStore((s) => s.loadErrors.endpoints);
  const actions = useActions();
  const dispatch = useDispatch();
  const toNetwork = () => dispatch({ type: "drawer", drawer: { kind: "system", tab: "network" } });
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
    <>
      {loadError && (
        <Body>
          <InlineError error={loadError} onRetry={() => void actions.refreshEndpoints()} />
        </Body>
      )}
      {listed.length > 0 && <Roles view={view} onNetwork={toNetwork} />}
      <Section title={t("endpoints.list")}>
        {open ? (
          <Body>
            <div className="card add">
              {view.profiles.length === 0 && <p className="muted">{t("endpoints.empty")}</p>}
              <EndpointEditor view={view} focus={adding} onSaved={setDraft} onNetwork={toNetwork} onDone={close} onCancel={view.profiles.length > 0 ? close : undefined} />
            </div>
          </Body>
        ) : (
          <Row kind="button" icon="plus" label={t("endpoints.add")} onClick={() => setAdding(true)} />
        )}
        <Body>
          <ul className="profiles">
            {listed.map((p) => (
              <ProfileCard key={p.profile.name} view={p} all={view} onNetwork={toNetwork} />
            ))}
          </ul>
        </Body>
      </Section>
    </>
  );
}

function ProfileCard({ view, all, onNetwork }: { view: ProfileView; all: EndpointsView; onNetwork: () => void }) {
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
        <EndpointEditor view={all} profile={view} focus onNetwork={onNetwork} onDone={() => setEditing(false)} onCancel={() => setEditing(false)} />
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
function Roles({ view, onNetwork }: { view: EndpointsView; onNetwork: () => void }) {
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
    <Section className="roles" title={t("endpoints.roles")}>
      {rows.map(({ kind, role, none, reasoning }) => {
        const label = t(`endpoints.role.${kind}`);
        const current = roles?.[kind] ?? null;
        // Without a chat role nothing else can be set.
        const off = !roles && kind !== "chat";
        return (
          <div key={kind} className="role">
            <Field label={label} htmlFor={`${id}-${kind}`}>
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
            </Field>
            {reasoning && (
              <Field label={t("endpoints.reasoning")} htmlFor={`${id}-${kind}-r`}>
                <select id={`${id}-${kind}-r`} aria-label={t("endpoints.reasoning.of", { role: label })} disabled={!current} value={current?.reasoning ?? ""} onChange={(e) => current && void setRole(kind, { ...current, reasoning: e.target.value || null })}>
                  <option value="">{t("endpoints.reasoning.default")}</option>
                  {REASONING.map((r) => (
                    <option key={r} value={r}>
                      {t(`endpoints.reasoning.${r}`)}
                    </option>
                  ))}
                </select>
              </Field>
            )}
            {current && kind !== "tts" && (
              <Body>
                <EndpointTester profile={current.profile} model={current.model} label={t("endpoints.test.of", { role: label })} onNetwork={onNetwork} />
              </Body>
            )}
          </div>
        );
      })}
      {options.length === 0 && <Note>{t("endpoints.roles.empty")}</Note>}
      <Note>{t("endpoints.reasoning.hint")}</Note>
    </Section>
  );
}
