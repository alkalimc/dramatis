import { Fragment, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Icon, type IconName } from "./Icon";

export interface MenuItem {
  label: string;
  icon?: IconName;
  danger?: boolean;
  /** A band between this item and the one above it. */
  separated?: boolean;
  onSelect: () => void;
}

/**
 * A small menu opened at a point (right click) or under a button. Escape, a click outside
 * or a scroll closes it; arrows, Home and End move between items.
 */
export function Menu({ at, items, label, onClose }: { at: { x: number; y: number }; items: MenuItem[]; label: string; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState(at);

  // Keep the whole menu on screen: flip it back from the right and bottom edges.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    setPos({
      x: Math.max(4, Math.min(at.x, window.innerWidth - r.width - 4)),
      y: Math.max(4, Math.min(at.y, window.innerHeight - r.height - 4)),
    });
  }, [at]);

  useEffect(() => {
    ref.current?.querySelector<HTMLElement>("button")?.focus();
    // Some webviews report a blur as a right click lands; only a blur that follows is a real one.
    const openedAt = performance.now();
    const leave = () => {
      if (performance.now() - openedAt > 400) onClose();
    };
    const away = (e: Event) => {
      // A right click that another handler took (it opened its own menu) is not ours to close on.
      if (e.type === "contextmenu" && e.defaultPrevented) return;
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    // The event that opened the menu is still travelling up the page; listening from the next
    // tick keeps it from closing the menu it just opened.
    const arm = setTimeout(() => {
      document.addEventListener("mousedown", away);
      document.addEventListener("contextmenu", away);
      window.addEventListener("blur", leave);
      window.addEventListener("resize", onClose);
    }, 0);
    return () => {
      clearTimeout(arm);
      document.removeEventListener("mousedown", away);
      document.removeEventListener("contextmenu", away);
      window.removeEventListener("blur", leave);
      window.removeEventListener("resize", onClose);
    };
  }, [onClose]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      onClose();
      return;
    }
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) return;
    e.preventDefault();
    const all = [...(ref.current?.querySelectorAll<HTMLElement>("button") ?? [])];
    const at = all.indexOf(document.activeElement as HTMLElement);
    const next = e.key === "Home" ? 0 : e.key === "End" ? all.length - 1 : (at + (e.key === "ArrowDown" ? 1 : all.length - 1)) % all.length;
    all[next]?.focus();
  };

  return (
    <div ref={ref} className="menu-pop" role="menu" aria-label={label} style={{ left: pos.x, top: pos.y }} onKeyDown={onKeyDown}>
      {items.map((it) => (
        <Fragment key={it.label}>
          {it.separated && <div className="menu-sep" role="separator" />}
          <button
            type="button"
            role="menuitem"
            className={it.danger ? "danger" : undefined}
            onClick={() => {
              onClose();
              it.onSelect();
            }}
          >
            {it.icon && <Icon name={it.icon} size={20} />}
            <span>{it.label}</span>
          </button>
        </Fragment>
      ))}
    </div>
  );
}
