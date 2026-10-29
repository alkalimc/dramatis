import type { ReactNode } from "react";
import { useActions, useApi } from "../store";

/**
 * A link to a page on the web. The app's webview opens nothing for a new-window link, so
 * inside it the address goes to the system browser through the engine; in a plain browser
 * (preview, tests) the link works as written.
 */
export function ExternalLink({ href, children }: { href: string; children: ReactNode }) {
  const api = useApi();
  const actions = useActions();
  return (
    <a
      href={href}
      target="_blank"
      rel="noopener noreferrer"
      onClick={(e) => {
        if (api.mock) return;
        e.preventDefault();
        void actions.attempt(api.commands.openExternal(href));
      }}
    >
      {children}
    </a>
  );
}
