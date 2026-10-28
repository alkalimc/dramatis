import { useEffect, useId, useState } from "react";
import type { QuietHours, QuotaStatus, Settings, Tier } from "../api";
import { useT, userTitle } from "../i18n";
import { when } from "../lib/format";
import { quotaExhaustedLine } from "../lib/quota";
import { useActions, useApi, useStore } from "../store";

export const TIERS: Tier[] = ["low", "middle", "high", "extra_high", "max", "ultra"];

/** The one quota control, the rolling hour, and which band they put us in. */
export function QuotaPanel({ quota, onTier }: { quota: QuotaStatus; onTier: (tier: Tier) => void }) {
  const t = useT();
  const id = useId();
  const exhaustedLine = quotaExhaustedLine(quota, t);
  const total = quota.spent.reduce((n, s) => n + (s.calls ?? 0), 0);

  return (
    <section className="quota" aria-labelledby={`${id}-h`}>
      <h3 id={`${id}-h`}>{t("quota.title")}</h3>
      <fieldset className="mode mode-none tier">
        <legend>{t("quota.tier")}</legend>
        <div className="mode-options">
          {TIERS.map((tier) => (
            <label key={tier} className="mode-choice">
              <input type="radio" name={id} value={tier} checked={tier === quota.tier} onChange={() => onTier(tier)} />
              <span>{t(`quota.tier.${tier}`)}</span>
            </label>
          ))}
        </div>
      </fieldset>

      {quota.tier === "ultra" ? (
        <p className="band">{t("quota.ultra")}</p>
      ) : (
        <>
          {quota.window && (
            <dl className="windows">
              <div className="window">
                <dt>{t("quota.window")}</dt>
                <dd>
                  <meter min={0} max={1} low={0.8} high={0.99} optimum={0} value={Math.min(quota.window.used ?? 0, 1)} aria-label={t("quota.window")} />
                  <span>{t("quota.used", { percent: Math.round((quota.window.used ?? 0) * 100) })}</span>
                </dd>
              </div>
            </dl>
          )}
          <p className={`band band-${quota.band}`} data-band={quota.band}>
            {t(`quota.band.${quota.band}`)}
          </p>
          {exhaustedLine && <p className="host-line">{exhaustedLine}</p>}
          {quota.band !== "open" && quota.window?.release_at && <p className="muted">{t("quota.release", { when: when(quota.window.release_at) })}</p>}
        </>
      )}

      <h4>{t("quota.spent")}</h4>
      {total === 0 ? (
        <p className="muted">{t("quota.spent.empty")}</p>
      ) : (
        <ul className="spend">
          {quota.spent.map((s) => (
            <li key={s.shape}>
              <span>{t(`quota.shape.${s.shape}`)}</span>
              <span className="muted">{Math.round(((s.calls ?? 0) / total) * 100)}%</span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function QuietPanel({ settings, save }: { settings: Settings; save: (s: Settings) => void }) {
  const t = useT();
  const id = useId();
  // The engine fills the default hours on first run; re-enabling restores the last ones.
  const [last, setLast] = useState<QuietHours>(settings.quiet_hours ?? { from: "", to: "" });
  const [on, setOn] = useState(settings.quiet_hours !== null);
  const hours = settings.quiet_hours ?? last;
  const update = (next: QuietHours) => {
    setLast(next);
    if (next.from && next.to) save({ ...settings, quiet_hours: next });
  };
  return (
    <section className="quiet" aria-labelledby={`${id}-h`}>
      <h3 id={`${id}-h`}>{t("quiet.title")}</h3>
      <label className="switch">
        <input
          type="checkbox"
          role="switch"
          checked={on}
          onChange={(e) => {
            setOn(e.target.checked);
            if (!e.target.checked) save({ ...settings, quiet_hours: null });
            else if (last.from && last.to) save({ ...settings, quiet_hours: last });
          }}
        />
        <span>{t("quiet.dnd")}</span>
      </label>
      {on && (
        <div className="field-row">
          <label className="field">
            <span>{t("quiet.from")}</span>
            <input type="time" value={hours.from} onChange={(e) => update({ ...hours, from: e.target.value })} />
          </label>
          <label className="field">
            <span>{t("quiet.to")}</span>
            <input type="time" value={hours.to} onChange={(e) => update({ ...hours, to: e.target.value })} />
          </label>
        </div>
      )}
      <label className="switch">
        <input
          type="checkbox"
          role="switch"
          checked={settings.notifications}
          aria-describedby={`${id}-n`}
          onChange={(e) => save({ ...settings, notifications: e.target.checked })}
        />
        <span>{t("quiet.notifications")}</span>
      </label>
      <p id={`${id}-n`} className="muted small">
        {t("quiet.notifications.hint")}
      </p>
    </section>
  );
}

export function YouPanel({ settings, save }: { settings: Settings; save: (s: Settings) => Promise<boolean> }) {
  const t = useT();
  const id = useId();
  const [name, setName] = useState(settings.user_name ?? "");
  const [month, setMonth] = useState(settings.birthday ? String(settings.birthday.month) : "");
  const [day, setDay] = useState(settings.birthday ? String(settings.birthday.day) : "");
  const [saved, setSaved] = useState(false);
  return (
    <section className="you" aria-labelledby={`${id}-h`}>
      <h3 id={`${id}-h`}>{t("you.title")}</h3>
      <form
        onSubmit={async (e) => {
          e.preventDefault();
          if (!name.trim()) return;
          const ok = await save({ ...settings, user_name: name.trim(), birthday: birthday(month, day) });
          setSaved(ok);
        }}
      >
        <NameAndBirthday id={id} name={name} setName={setName} month={month} setMonth={setMonth} day={day} setDay={setDay} />
        <p className="muted">{t("you.preview", { title: userTitle(t, name) })}</p>
        <p className="row form-actions">
          <button type="submit" disabled={!name.trim()}>
            {t("you.save")}
          </button>
          <span role="status" className="muted">
            {saved ? t("you.saved") : ""}
          </span>
        </p>
      </form>
    </section>
  );
}

export function birthday(month: string, day: string): Settings["birthday"] {
  const m = Number.parseInt(month, 10);
  const d = Number.parseInt(day, 10);
  if (!(m >= 1 && m <= 12 && d >= 1 && d <= 31)) return null;
  return { month: m, day: d };
}

export function NameAndBirthday(p: {
  id: string;
  name: string;
  setName: (v: string) => void;
  month: string;
  setMonth: (v: string) => void;
  day: string;
  setDay: (v: string) => void;
  nameLabel?: string;
  birthdayLabel?: string;
}) {
  const t = useT();
  return (
    <>
      <label htmlFor={`${p.id}-name`}>{p.nameLabel ?? t("you.name")}</label>
      <input id={`${p.id}-name`} value={p.name} required autoComplete="nickname" onChange={(e) => p.setName(e.target.value)} />
      <fieldset className="birthday">
        <legend>{p.birthdayLabel ?? t("you.birthday")}</legend>
        <label className="field">
          <span>{t("you.month")}</span>
          <input type="number" min={1} max={12} inputMode="numeric" value={p.month} onChange={(e) => p.setMonth(e.target.value)} />
        </label>
        <label className="field">
          <span>{t("you.day")}</span>
          <input type="number" min={1} max={31} inputMode="numeric" value={p.day} onChange={(e) => p.setDay(e.target.value)} />
        </label>
      </fieldset>
    </>
  );
}

/** The quota page of the settings card; loads the numbers when it opens. */
export function QuotaSection() {
  const api = useApi();
  const actions = useActions();
  const quota = useStore((s) => s.quota);

  useEffect(() => {
    void actions.refreshQuota();
  }, [actions]);

  return quota ? (
    <QuotaPanel
      quota={quota}
      onTier={async (tier) => {
        if ((await actions.attempt(api.commands.setTier(tier))) !== undefined) void actions.refreshQuota();
      }}
    />
  ) : null;
}

export function QuietSection() {
  const actions = useActions();
  const settings = useStore((s) => s.settings);
  return settings ? <QuietPanel settings={settings} save={(s) => void actions.saveSettings(s)} /> : null;
}

export function YouSection() {
  const actions = useActions();
  const settings = useStore((s) => s.settings);
  return settings ? <YouPanel settings={settings} save={actions.saveSettings} /> : null;
}
