import { useId, useMemo, useState } from "react";
import type { PersonRef } from "../api";
import { useT } from "../i18n";
import { HOST_ID } from "../ui/Avatar";
import { useStore } from "../store";

/**
 * Pick someone from the roster by name. Disabled people are not offered: for any purpose
 * that reaches a person, they do not exist.
 */
export function PersonPicker({
  label,
  exclude = [],
  onPick,
  withHost = false,
}: {
  label: string;
  exclude?: string[];
  onPick: (p: PersonRef) => void;
  /** Offer the host too (a group can have it as its assistant). */
  withHost?: boolean;
}) {
  const t = useT();
  const id = useId();
  const roster = useStore((s) => s.roster);
  const [q, setQ] = useState("");
  const people = useMemo(
    () =>
      (roster?.groups ?? [])
        .filter((g) => g.band !== "disabled")
        .flatMap((g) => g.rows)
        .filter((r) => r.mode !== "disabled" && !exclude.includes(r.person.id))
        .map((r) => r.person),
    [roster, exclude],
  );
  const host = t("host.name");
  const all = withHost && !exclude.includes(HOST_ID) ? [{ id: HOST_ID, name: host }, ...people] : people;
  const shown = all.filter((p) => p.name.toLowerCase().includes(q.trim().toLowerCase())).slice(0, 8);
  return (
    <div className="picker">
      <label htmlFor={id}>{label}</label>
      <input id={id} type="search" value={q} placeholder={t("person_picker.search")} onChange={(e) => setQ(e.target.value)} />
      <ul className="picker-list">
        {shown.map((p) => (
          <li key={p.id}>
            <button type="button" className="chip" onClick={() => onPick(p)}>
              {p.name}
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
