import { useEffect, useState, type RefObject } from "react";
import { useT } from "../i18n";
import { Icon } from "./Icon";

/** How far down a pane must be scrolled before the button shows. */
const FAR = 240;

/**
 * A round button at the bottom right of a pane whose controls sit at its top; it shows once the
 * pane is scrolled down and takes it back up. Put it beside the scrolling element, not inside.
 */
export function BackToTop({ scroller }: { scroller: RefObject<HTMLElement | null> }) {
  const t = useT();
  const [far, setFar] = useState(false);
  useEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const on = () => setFar(el.scrollTop > FAR);
    on();
    el.addEventListener("scroll", on, { passive: true });
    return () => el.removeEventListener("scroll", on);
  }, [scroller]);
  if (!far) return null;
  return (
    <button type="button" className="to-top round" aria-label={t("page.top")} title={t("page.top")} onClick={() => scroller.current?.scrollTo?.({ top: 0, behavior: "smooth" })}>
      <Icon name="up" size={24} />
    </button>
  );
}
