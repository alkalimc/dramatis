import type { PersonRef } from "../api";
import { Avatar } from "../ui/Avatar";

/**
 * People as a list to browse, picture and name per row, like the roster and the contacts:
 * one press takes the person. The list scrolls inside itself, so a long roster fits a dialog.
 */
export function PeopleList({
  people,
  label,
  take,
  empty,
  onPick,
}: {
  people: PersonRef[];
  label: string;
  /** The accessible name of a row's button, when it should say more than the name. */
  take?: (p: PersonRef) => string;
  /** Said when there is nobody to list. */
  empty: string;
  onPick: (p: PersonRef) => void;
}) {
  if (people.length === 0) return <p className="muted small">{empty}</p>;
  return (
    <ul className="people-list" aria-label={label}>
      {people.map((p) => (
        <li key={p.id}>
          <button type="button" className="contact-row" aria-label={take?.(p)} onClick={() => onPick(p)}>
            <Avatar person={p} />
            <span className="chat-name">{p.name}</span>
          </button>
        </li>
      ))}
    </ul>
  );
}
