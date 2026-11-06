import { useCallback, useEffect, useState } from "react";
import { settle, type Failure, type PluginInfo } from "../api";
import { useT } from "../i18n";
import { useApi, useStore } from "../store";
import { InlineError } from "../ui/ErrorBar";
import { Note, Row, Section } from "../ui/SettingsKit";

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
    <>
      <Section>
        <Note>{t("plugins.intro")}</Note>
        <Note>{t("plugins.warn")}</Note>
        {home && <Note>{t("plugins.folder", { path: `${home}/plugins` })}</Note>}
      </Section>
      <Section>
        {list.length === 0 && <Note role="status">{t("plugins.none")}</Note>}
        {list.map((p) => (
          <div key={p.name} className="plugin">
            <Row
              kind="toggle"
              icon="plugin"
              label={p.name}
              hint={[p.version, p.description].filter(Boolean).join(" ") || undefined}
              checked={p.enabled}
              disabled={p.problem !== null && !p.enabled}
              onChange={(on) => void toggle(p.name, on)}
            />
            {p.hooks.length > 0 && <Note>{t("plugins.hooks", { list: p.hooks.join(", ") })}</Note>}
            {p.tools.length > 0 && <Note>{t("plugins.tools", { list: p.tools.join(", ") })}</Note>}
            {p.problem && <p className="inline-error sx-note">{t("plugins.problem", { detail: p.problem })}</p>}
            {p.status && <p className="inline-error sx-note">{t("plugins.status", { detail: p.status })}</p>}
          </div>
        ))}
        {error && (
          <div className="sx-body">
            <InlineError error={error} />
          </div>
        )}
        <Note>{t("plugins.hint")}</Note>
      </Section>
    </>
  );
}
