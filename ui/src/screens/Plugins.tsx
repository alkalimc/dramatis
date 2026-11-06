import { useCallback, useEffect, useState } from "react";
import { settle, type Failure, type PluginInfo } from "../api";
import { useT } from "../i18n";
import { useApi, useStore } from "../store";
import { InlineError } from "../ui/ErrorBar";

/** The plugins in the home's plugins folder, each with an on/off switch. Off until turned on. */
export function PluginsPage() {
  const t = useT();
  const api = useApi();
  const home = useStore((s) => s.status?.home_dir);
  const [list, setList] = useState<PluginInfo[] | null>(null);
  const [error, setError] = useState<Failure | null>(null);

  const load = useCallback(async () => {
    const r = await settle(api.commands.plugins());
    if (r.ok) setList(r.data);
    else setError(r.error);
  }, [api]);

  useEffect(() => {
    void load();
  }, [load]);

  const toggle = async (name: string, enabled: boolean) => {
    setError(null);
    const r = await settle(api.commands.setPluginEnabled(name, enabled));
    if (!r.ok) setError(r.error);
    await load();
  };

  if (list === null) return error ? <InlineError error={error} /> : <p className="muted" role="status">{t("app.loading")}</p>;

  return (
    <section>
      <p className="muted small">{t("plugins.intro")}</p>
      <p className="muted small">{t("plugins.warn")}</p>
      {home && <p className="muted small">{t("plugins.folder", { path: `${home}/plugins` })}</p>}
      {list.length === 0 && <p role="status">{t("plugins.none")}</p>}
      <ul className="plain">
        {list.map((p) => (
          <li key={p.name}>
            <label className="switch">
              <span>
                {p.name} {p.version && <span className="muted small">{p.version}</span>}
                {p.description && <span className="muted small"> {p.description}</span>}
              </span>
              <input type="checkbox" role="switch" aria-label={p.name} checked={p.enabled} disabled={p.problem !== null && !p.enabled} onChange={(e) => void toggle(p.name, e.target.checked)} />
            </label>
            {p.hooks.length > 0 && <p className="muted small">{t("plugins.hooks", { list: p.hooks.join(", ") })}</p>}
            {p.tools.length > 0 && <p className="muted small">{t("plugins.tools", { list: p.tools.join(", ") })}</p>}
            {p.problem && <p className="inline-error">{t("plugins.problem", { detail: p.problem })}</p>}
            {p.status && <p className="inline-error">{t("plugins.status", { detail: p.status })}</p>}
          </li>
        ))}
      </ul>
      {error && <InlineError error={error} />}
      <p className="muted small">{t("plugins.hint")}</p>
    </section>
  );
}
