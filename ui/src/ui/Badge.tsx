import type { ReactNode } from "react";

/** A count past this reads as "99+": the pill stays a circle for one or two digits. */
const CAP = 99;

export function badgeText(count: number): string {
  return count > CAP ? `${CAP}+` : String(count);
}

type Props = {
  count: number;
  /** Grey instead of accent: for a muted or archived conversation. */
  quiet?: boolean;
  className?: string;
  label?: string;
  title?: string;
  /** Makes the badge a button. */
  onClick?: () => void;
  decorative?: boolean;
};

/** The one unread pill: rail, chat list and the jump-to-latest button all use it. */
export function Badge({ count, quiet, className = "", label, title, onClick, decorative }: Props): ReactNode {
  const text = badgeText(count);
  const cls = `badge${text.length > 2 ? " wide" : ""}${quiet ? " quiet" : ""}${className ? ` ${className}` : ""}`;
  if (onClick) {
    return (
      <button type="button" className={cls} aria-label={label} title={title} onClick={onClick}>
        {text}
      </button>
    );
  }
  return (
    <span className={cls} aria-hidden={decorative ? true : undefined} aria-label={label} title={title}>
      {text}
    </span>
  );
}
