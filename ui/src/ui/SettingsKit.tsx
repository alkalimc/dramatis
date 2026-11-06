import { useId, type ReactNode } from "react";
import { Icon, type IconName } from "./Icon";

/**
 * The parts every settings page is built from, so that all of them look and behave alike:
 * a page (the scrolling body of the card), sections (an accent title over rows, one band
 * between sections), and rows of [icon · label · value or control at the right].
 */

/** The body of one page in the settings card: it scrolls inside the card, which never changes size. */
export function SettingsPage({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="settings-page" role="region" aria-label={label}>
      {children}
    </div>
  );
}

/** A titled group of rows. The title is optional for a page that is one group. */
export function Section({ title, children, className = "" }: { title?: string; children: ReactNode; className?: string }) {
  // Not a landmark: the page is the region, a section is only a visual group under its heading.
  return (
    <section className={`sx ${className}`.trim()}>
      {title && <h3 className="sx-title">{title}</h3>}
      {children}
    </section>
  );
}

/** Small print under rows: what a setting does. */
export function Note({ children, id, role }: { children: ReactNode; id?: string; role?: "status" | "alert" }) {
  return (
    <p id={id} role={role} className="sx-note">
      {children}
    </p>
  );
}

/** A block of free content (a form, a chart, a card) that sits in the section's gutter. */
export function Body({ children, className = "" }: { children: ReactNode; className?: string }) {
  return <div className={`sx-body ${className}`.trim()}>{children}</div>;
}

type Common = { icon?: IconName; label: ReactNode; hint?: ReactNode };
export type RowProps = Common &
  (
    | { kind?: "nav"; value?: string; onClick: () => void }
    | { kind: "button"; onClick: () => void; tone?: "accent" | "danger"; disabled?: boolean; busy?: boolean }
    | { kind: "toggle"; checked: boolean; onChange: (on: boolean) => void; disabled?: boolean; name?: string }
    | { kind: "check"; checked: boolean; onChange: (on: boolean) => void; disabled?: boolean }
    | { kind: "choice"; group: string; checked: boolean; onChange: () => void }
    | { kind: "info"; value?: ReactNode }
  );

/**
 * One row. `nav` opens something (value and chevron at the right), `button` does something,
 * `toggle` is a switch, `check` a checkbox, `choice` one radio of a group, `info` shows a value.
 */
export function Row(props: RowProps) {
  const labelId = useId();
  const hintId = useId();
  const { icon, label, hint } = props;
  const lead = <span className="sx-lead">{icon && <Icon name={icon} size={24} />}</span>;
  const text = (
    <span className="sx-text">
      <span id={labelId} className="sx-label">
        {label}
      </span>
      {hint && (
        <span id={hintId} className="sx-hint">
          {hint}
        </span>
      )}
    </span>
  );
  const kind = props.kind ?? "nav";

  if (kind === "nav") {
    const p = props as Extract<RowProps, { kind?: "nav" }>;
    return (
      <button type="button" className="sx-row" onClick={p.onClick}>
        {lead}
        {text}
        {p.value && <span className="sx-value">{p.value}</span>}
        <span className="sx-go">
          <Icon name="chevron" size={18} />
        </span>
      </button>
    );
  }
  if (kind === "button") {
    const p = props as Extract<RowProps, { kind: "button" }>;
    return (
      <button type="button" className={`sx-row sx-act${p.tone === "danger" ? " sx-danger" : ""}`} disabled={p.disabled} aria-busy={p.busy || undefined} onClick={p.onClick}>
        {lead}
        {text}
      </button>
    );
  }
  if (kind === "info") {
    const p = props as Extract<RowProps, { kind: "info" }>;
    return (
      <div className="sx-row sx-plain">
        {lead}
        {text}
        {p.value !== undefined && <span className="sx-value">{p.value}</span>}
      </div>
    );
  }
  const p = props as Extract<RowProps, { kind: "toggle" | "check" | "choice" }>;
  const input =
    p.kind === "toggle" ? (
      <input type="checkbox" role="switch" name={p.name} checked={p.checked} disabled={p.disabled} aria-labelledby={labelId} aria-describedby={hint ? hintId : undefined} onChange={(e) => p.onChange(e.target.checked)} />
    ) : p.kind === "check" ? (
      <input type="checkbox" checked={p.checked} disabled={p.disabled} aria-labelledby={labelId} aria-describedby={hint ? hintId : undefined} onChange={(e) => p.onChange(e.target.checked)} />
    ) : (
      <input type="radio" name={p.group} checked={p.checked} aria-labelledby={labelId} aria-describedby={hint ? hintId : undefined} onChange={() => p.onChange()} />
    );
  return (
    <label className={`sx-row sx-${p.kind}`}>
      {lead}
      {text}
      {input}
    </label>
  );
}

/** The radios of one choice: a group with a name for screen readers, rows built with `kind: "choice"`. */
export function Choices({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div role="radiogroup" aria-label={label} className="sx-choices">
      {children}
    </div>
  );
}

/** A label over a control, in the section's gutter: for text, times and numbers. */
export function Field({ label, htmlFor, children, hint, hintId }: { label: ReactNode; htmlFor?: string; children: ReactNode; hint?: ReactNode; hintId?: string }) {
  return (
    <div className="sx-field">
      <label htmlFor={htmlFor}>{label}</label>
      {children}
      {hint && (
        <p id={hintId} className="sx-note sx-flush">
          {hint}
        </p>
      )}
    </div>
  );
}
