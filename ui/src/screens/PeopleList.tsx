import { useId, type KeyboardEvent, type ReactNode } from "react";
import type { PersonRef } from "../api";
import { Avatar } from "../ui/Avatar";
import { Icon } from "../ui/Icon";

/**
 * Everywhere the user chooses or browses people uses this family: the contacts card, the
 * forward and add-to-group pickers, the new-group dialog, a group's members, and the
 * @ mention popup. A row is [picture · name · second line], optionally with a control at
 * its end; a search field narrows the list; arrow keys walk the rows; Enter on the field
 * takes the first.
 */
export type Entry = {
  key: string;
  name: string;
  /** The picture; a person's by default. */
  avatar?: ReactNode;
  /** A line under the name. */
  secondary?: ReactNode;
  /** The accessible name of the row's button, when it should say more than the name. */
  take?: string;
  disabled?: boolean;
  /** A control at the row's end (remove, say); it is not part of the row's button. */
  trailing?: ReactNode;
  /** The thing the row stands for, handed back on pick. */
  value?: unknown;
};

export const personEntry = (p: PersonRef, extra: Partial<Entry> = {}): Entry => ({ key: p.id, name: p.name, value: p, ...extra });

type Size = "s" | "m" | "l";

/** The one row: picture, name, second line. */
export function PersonRow({
  entry,
  size = "m",
  onPick,
  onKeyDown,
  tabIndex,
  keepFocus = false,
}: {
  entry: Entry;
  size?: Size;
  onPick: () => void;
  onKeyDown?: (e: KeyboardEvent<HTMLButtonElement>) => void;
  tabIndex?: number;
  /** Pressing the row leaves focus where it is (the mention popup keeps the text field's). */
  keepFocus?: boolean;
}) {
  const person = (entry.value ?? { id: entry.key, name: entry.name }) as PersonRef;
  return (
    <button type="button" className={`person-row person-row-${size}`} aria-label={entry.take} disabled={entry.disabled} tabIndex={tabIndex} onClick={onPick} onKeyDown={onKeyDown} onMouseDown={keepFocus ? (e) => e.preventDefault() : undefined}>
      {entry.avatar ?? <Avatar person={person} size={size} />}
      <span className="person-text">
        <span className="person-name">{entry.name}</span>
        {entry.secondary && <span className="person-sub muted small">{entry.secondary}</span>}
      </span>
    </button>
  );
}

/** Up and down move between the rows of a list. */
function walk(e: KeyboardEvent<HTMLElement>) {
  if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
  const rows = [...(e.currentTarget.closest("ul")?.querySelectorAll<HTMLElement>("button.person-row") ?? [])];
  const at = rows.indexOf(e.currentTarget);
  const next = rows[at + (e.key === "ArrowDown" ? 1 : -1)];
  if (!next) return;
  e.preventDefault();
  next.focus();
}

/** People as rows to browse. The list scrolls inside itself, so a long roster fits a dialog. */
export function PeopleList({ entries, label, empty, size = "m", bare = false, onPick }: { entries: Entry[]; label: string; empty: string; size?: Size; bare?: boolean; onPick: (e: Entry) => void }) {
  if (entries.length === 0) return <p className="muted small pad">{empty}</p>;
  return (
    <ul className={`people-list${bare ? " bare" : ""}`} aria-label={label}>
      {entries.map((e) => (
        <li key={e.key}>
          <PersonRow entry={e} size={size} onPick={() => onPick(e)} onKeyDown={walk} />
          {e.trailing}
        </li>
      ))}
    </ul>
  );
}

/**
 * A search field over a list. `enter` is what Enter does in the field: `"first"` takes the
 * first row, a function runs instead (a search by topic, say). Down arrow goes to the rows.
 */
export function PeopleSearch({
  value,
  onChange,
  placeholder,
  label,
  listLabel,
  entries,
  empty,
  size = "m",
  enter,
  action,
  autoFocus = false,
  onPick,
}: {
  value: string;
  onChange: (q: string) => void;
  placeholder: string;
  /** The field's name for screen readers; the placeholder when absent. */
  label?: string;
  listLabel: string;
  entries: Entry[];
  empty: string;
  size?: Size;
  enter?: "first" | (() => void);
  /** A button beside the field. */
  action?: ReactNode;
  autoFocus?: boolean;
  onPick: (e: Entry) => void;
}) {
  const id = useId();
  const first = entries.find((e) => !e.disabled);
  return (
    <div className="people-search">
      <div className="people-search-bar">
        <Icon name="search" size={20} />
        <label htmlFor={id} className="sr-only">
          {label ?? placeholder}
        </label>
        <input
          id={id}
          type="search"
          autoFocus={autoFocus}
          value={value}
          placeholder={placeholder}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && enter) {
              e.preventDefault();
              if (enter === "first") {
                if (first) onPick(first);
              } else enter();
            } else if (e.key === "ArrowDown") {
              e.preventDefault();
              e.currentTarget.closest(".people-search")?.querySelector<HTMLElement>("button.person-row:not(:disabled)")?.focus();
            }
          }}
        />
        {action}
      </div>
      <PeopleList entries={entries} label={listLabel} empty={empty} size={size} bare onPick={onPick} />
    </div>
  );
}

/** The people chosen so far, each one press from being taken out again. */
export function PickedChips({ people, remove, onRemove }: { people: PersonRef[]; remove: (p: PersonRef) => string; onRemove: (p: PersonRef) => void }) {
  if (people.length === 0) return null;
  return (
    <ul className="chips" aria-live="polite">
      {people.map((p) => (
        <li key={p.id}>
          <button type="button" className="chip on" aria-label={remove(p)} onClick={() => onRemove(p)}>
            {p.name} ×
          </button>
        </li>
      ))}
    </ul>
  );
}
