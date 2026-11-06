import { useCallback, useEffect, useId, useMemo, useState } from "react";
import type { StorageKind, StorageUsage } from "../api";
import { useT, type T } from "../i18n";
import { useActions, useApi } from "../store";
import { Note, Row, Section } from "../ui/SettingsKit";
import { LocationForm } from "./Location";

/** Kilobytes as a short size, in the corpus's own unit words. */
export function sizeText(t: T, kb: number): string {
  if (kb < 1024) return t("storage.kb", { n: kb });
  if (kb < 1024 * 1024) return t("storage.mb", { n: (kb / 1024).toFixed(1) });
  return t("storage.gb", { n: (kb / 1024 / 1024).toFixed(1) });
}

const R = 15.9155; // a circle whose circumference is 100, so a dash length is a percent
const GAP = 0.6;

/** A ring of the shares, drawn in plain SVG; the legend beside it carries the same numbers in words. */
function Donut({ parts, label, centre }: { parts: { kind: StorageKind; pct: number }[]; label: string; centre: string }) {
  let at = 0;
  return (
    <div className="storage-donut" role="img" aria-label={label}>
      <svg viewBox="0 0 42 42" aria-hidden="true">
        <circle className="storage-track" cx="21" cy="21" r={R} />
        {parts.map((p) => {
          const len = Math.max(p.pct - (parts.length > 1 ? GAP : 0), 0.1);
          const el = <circle key={p.kind} className={`storage-arc storage-k-${p.kind}`} cx="21" cy="21" r={R} strokeDasharray={`${len} ${100 - len}`} strokeDashoffset={-at} />;
          at += p.pct;
          return el;
        })}
      </svg>
      <span className="storage-centre" aria-hidden="true">
        {centre}
      </span>
    </div>
  );
}

/** Advanced, data and storage: where the files are kept, then what they are. */
export function StoragePage() {
  return (
    <>
      <LocationForm />
      <Usage />
    </>
  );
}

/**
 * What the app keeps on disk by kind, with the caches (which are fetched or made again
 * when needed) clearable. Saves and the corpus are only shown.
 */
function Usage() {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const id = useId();
  const [usage, setUsage] = useState<StorageUsage | null>(null);
  const [off, setOff] = useState<Set<StorageKind>>(new Set());
  const [note, setNote] = useState("");

  const refresh = useCallback(async () => {
    const u = await actions.attempt(api.commands.storageUsage());
    if (u) setUsage(u);
  }, [actions, api]);
  useEffect(() => {
    void refresh();
  }, [refresh]);

  const picked = useMemo(() => usage?.categories.filter((c) => c.clearable && !off.has(c.kind)) ?? [], [usage, off]);
  const pickedKb = picked.reduce((sum, c) => sum + c.kb, 0);

  if (!usage) return <Section className="storage">{null}</Section>;

  const shown = usage.categories.filter((c) => c.kb > 0);
  const total = Math.max(usage.total_kb, 1);
  const pct = (kb: number) => Math.round((kb / total) * 100);
  const kinds = (k: StorageKind) => t(`storage.kind.${k}`);

  const clear = async () => {
    if (!window.confirm(t("storage.clear.confirm", { size: sizeText(t, pickedKb) }))) return;
    const next = await actions.attempt(api.commands.clearStorage(picked.map((c) => c.kind)));
    if (!next) return;
    setUsage(next);
    setNote(t("storage.cleared", { size: sizeText(t, pickedKb) }));
  };

  return (
    <>
      <Section className="storage" title={t("storage.title")}>
        <Donut
          parts={shown.map((c) => ({ kind: c.kind, pct: (c.kb / total) * 100 }))}
          label={t("storage.chart", { parts: shown.map((c) => `${kinds(c.kind)} ${pct(c.kb)}%`).join(", ") })}
          centre={sizeText(t, usage.total_kb)}
        />
        <ul className="storage-list">
          {usage.categories.map((c) => (
            <li key={c.kind} className="storage-row">
              {c.clearable ? (
                <input
                  type="checkbox"
                  id={`${id}-${c.kind}`}
                  className={`storage-check storage-k-${c.kind}`}
                  checked={!off.has(c.kind)}
                  onChange={(e) =>
                    setOff((cur) => {
                      const next = new Set(cur);
                      if (e.target.checked) next.delete(c.kind);
                      else next.add(c.kind);
                      return next;
                    })
                  }
                />
              ) : (
                <span className={`storage-swatch storage-k-${c.kind}`} aria-hidden="true" />
              )}
              <label htmlFor={c.clearable ? `${id}-${c.kind}` : undefined} className="storage-what">
                <span className="storage-name">
                  {kinds(c.kind)} <span className="storage-pct">{pct(c.kb)}%</span>
                </span>
                <span className="muted small">
                  {t(`storage.note.${c.kind}`)} · {t("storage.files", { count: c.files })}
                </span>
              </label>
              <span className="storage-size">{sizeText(t, c.kb)}</span>
            </li>
          ))}
        </ul>
        <Row kind="button" icon="trash" tone="danger" disabled={pickedKb === 0} label={pickedKb === 0 ? t("storage.nothing") : t("storage.clear", { size: sizeText(t, pickedKb) })} onClick={() => void clear()} />
        <Note role="status">{note || t("storage.hint")}</Note>
      </Section>

      <Section title={t("storage.saves")}>
        {usage.saves.map((s) => (
          <Row key={s.name} kind="info" icon="bookmark" label={s.name} hint={s.current ? t("saves.current") : undefined} value={sizeText(t, s.kb)} />
        ))}
        <Note>{t("storage.pictures", { count: usage.pictures, size: sizeText(t, usage.picture_kb) })}</Note>
      </Section>

      <Section title={t("storage.limit.title")}>
        <Note>{t("storage.limit", { size: sizeText(t, usage.cache_limit_kb) })}</Note>
      </Section>
    </>
  );
}
