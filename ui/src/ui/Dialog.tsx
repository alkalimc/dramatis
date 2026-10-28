import { useEffect, useId, useRef, type ReactNode } from "react";

/**
 * A modal card: labelled, Escape closes, focus moves in on open and back on close, Tab
 * stays inside.
 */
export function Dialog({ title, onClose, children, className = "", hideTitle = false }: { title: string; onClose: () => void; children: ReactNode; className?: string; hideTitle?: boolean }) {
  const id = useId();
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const before = document.activeElement as HTMLElement | null;
    // A field that took focus itself (autoFocus) keeps it.
    if (ref.current?.contains(document.activeElement)) return () => before?.focus?.();
    // A text field, else the dialog itself: a ring on a button or switch reads as a stray highlight.
    const first = ref.current?.querySelector<HTMLElement>("[autofocus], input:not([type=file], [type=checkbox], [type=radio]), textarea, select");
    (first ?? ref.current)?.focus();
    return () => before?.focus?.();
  }, []);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      onClose();
      return;
    }
    if (e.key !== "Tab" || !ref.current) return;
    const items = [...ref.current.querySelectorAll<HTMLElement>("input:not([type=file]), textarea, select, button, a[href], summary")].filter(
      (el) => !el.hasAttribute("disabled"),
    );
    if (items.length === 0) return;
    const [first, last] = [items[0], items[items.length - 1]];
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  };

  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div ref={ref} className={`dialog ${className}`} tabIndex={-1} role="dialog" aria-modal="true" aria-labelledby={id} onKeyDown={onKeyDown}>
        <h2 id={id} className={hideTitle ? "sr-only" : undefined}>
          {title}
        </h2>
        {children}
      </div>
    </div>
  );
}
