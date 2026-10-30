import { useT } from "../i18n";
import type { Delivery } from "../lib/ticks";

/** The delivery marks of the user's own message: one tick once a reply is under way, two when it is done. */
export function Ticks({ delivery }: { delivery: Delivery }) {
  const t = useT();
  if (delivery === 0) return null;
  return (
    <span className={`ticks ticks-${delivery}`} role="img" aria-label={t(delivery === 1 ? "message.status.sent" : "message.status.answered")}>
      <svg viewBox="0 0 24 24" width={18} height={18} fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" focusable="false">
        {delivery === 1 ? <path d="M5 12.5l4.5 4.5L19 7.5" /> : <path d="M1.5 12.5L6 17l9.5-9.5M8 15.8l1.2 1.2L21 7.5" />}
      </svg>
    </span>
  );
}
