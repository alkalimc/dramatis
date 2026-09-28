import { useEffect, useId, useState } from "react";
import type { QuotaStatus, Settings, Tier, WindowSpan } from "../api";
import { useT } from "../i18n";
import { when } from "../lib/format";
import { useActions, useApi, useStore } from "../store";

export const TIERS: Tier[] = ["low", "middle", "high", "extra_high", "max", "ultra"];

/** The one quota control, the two rolling windows, and which band they put us in. */
export function QuotaPanel({ quota, onTier }: { quota: QuotaStatus; onTier: (tier: Tier) => void }) {
  const t = useT();
  const id = useId();
  const index = TIERS.indexOf(quota.tier);
  const windowName = (span: WindowSpan | null) => t(`quota.window.${span ?? "five_hours"}`);
  const binding = quota.windows.find((w) => w.span === quota.binding);
  const total = quota.spent.reduce((n, s) => n + (s.points ?? 0), 0);

  return (
    <section className="quota" aria-labelledby={`${id}-h`}>
      <h3 id={`${id}-h`}>{t("quota.title")}</h3>
      <div className="tier">
        <label htmlFor={id}>{t("quota.tier")}</label>
        <input
          id={id}
          type="range"
          min={0}
          max={TIERS.length - 1}
          step={1}
          value={index}
          aria-valuetext={t(`quota.tier.${quota.tier}`)}
          onChange={(e) => onTier(TIERS[Number(e.target.value)])}
        />
        <ol className="tier-ticks" aria-hidden="true">
          {TIERS.map((tier) => (
            <li key={tier} className={tier === quota.tier ? "on" : undefined}>
              {t(`quota.tier.${tier}`)}
            </li>
          ))}
        </ol>
      </div>

      {quota.tier === "ultra" ? (
        <p className="band">{t("quota.ultra")}</p>
      ) : (
        <>
          <dl className="windows">
            {quota.windows.map((w) => {
              const used = w.used ?? 0;
              const pct = Math.round(used * 100);
              return (
                <div key={w.span} className="window">
                  <dt>{windowName(w.span)}</dt>
                  <dd>
                    <meter min={0} max={1} low={0.8} high={0.99} optimum={0} value={Math.min(used, 1)} aria-label={windowName(w.span)} />
                    <span>{t("quota.used", { percent: pct })}</span>
                  </dd>
                </div>
              );
            })}
          </dl>
          <p className={`band band-${quota.band}`} data-band={quota.band}>
            {t(`quota.band.${quota.band}`, { window: windowName(quota.binding) })}
          </p>
          {quota.band !== "open" && binding?.release_at && <p className="muted">{t("quota.release", { when: when(binding.release_at) })}</p>}
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
              <span className="muted">{Math.round(((s.points ?? 0) / total) * 100)}%</span>
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
  const quiet = settings.quiet_hours;
  return (
    <section className="quiet" aria-labelledby={`${id}-h`}>
      <h3 id={`${id}-h`}>{t("quiet.title")}</h3>
      <div className="row wrap">
        <label>
          <input
            type="checkbox"
            checked={quiet !== null}
            onChange={(e) => save({ ...settings, quiet_hours: e.target.checked ? { from: "23:00", to: "07:00" } : null })}
          />{" "}
          {t("quiet.dnd")}
        </label>
        {quiet && (
          <>
            <label>
              {t("quiet.from")} <input type="time" value={quiet.from} onChange={(e) => save({ ...settings, quiet_hours: { ...quiet, from: e.target.value } })} />
            </label>
            <label>
              {t("quiet.to")} <input type="time" value={quiet.to} onChange={(e) => save({ ...settings, quiet_hours: { ...quiet, to: e.target.value } })} />
            </label>
          </>
        )}
      </div>
      <label className="switch">
        <input
          type="checkbox"
          role="switch"
          checked={settings.notifications}
          aria-describedby={`${id}-n`}
          onChange={(e) => save({ ...settings, notifications: e.target.checked })}
        />{" "}
        {t("quiet.notifications")}
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
        <p className="row">
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
        <label>
          {t("you.month")} <input type="number" min={1} max={12} inputMode="numeric" value={p.month} onChange={(e) => p.setMonth(e.target.value)} />
        </label>
        <label>
          {t("you.day")} <input type="number" min={1} max={31} inputMode="numeric" value={p.day} onChange={(e) => p.setDay(e.target.value)} />
        </label>
      </fieldset>
    </>
  );
}

/** System drawer, first tab: quota, quiet hours and notifications, and the user. */
export function QuotaAndQuiet() {
  const api = useApi();
  const actions = useActions();
  const quota = useStore((s) => s.quota);
  const settings = useStore((s) => s.settings);

  useEffect(() => {
    void actions.refreshQuota();
  }, [actions]);

  return (
    <>
      {quota && (
        <QuotaPanel
          quota={quota}
          onTier={async (tier) => {
            if ((await actions.attempt(api.commands.setTier(tier))) !== undefined) void actions.refreshQuota();
          }}
        />
      )}
      {settings && (
        <>
          <QuietPanel settings={settings} save={(s) => void actions.saveSettings(s)} />
          <YouPanel settings={settings} save={actions.saveSettings} />
        </>
      )}
    </>
  );
}
