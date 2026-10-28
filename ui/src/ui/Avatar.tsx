import { useEffect, useState } from "react";
import type { PersonRef } from "../api";
import { useApi } from "../store";

// Fetched on demand, once per person per session; `null` = none, fall back to a letter.
const cache = new Map<string, Promise<string | null>>();

/** `plain` skips the portrait lookup: the initial only (the host, a group). */
export function Avatar({ person, size = "m", plain = false }: { person: PersonRef; size?: "s" | "m" | "l" | "xl"; plain?: boolean }) {
  const api = useApi();
  const [src, setSrc] = useState<string | null>(null);

  useEffect(() => {
    if (plain) return;
    let live = true;
    let p = cache.get(person.id);
    if (!p) {
      p = api.commands.avatar(person.id).then((r) => (r.status === "ok" && r.data ? api.mediaUrl(r.data.path) : null));
      cache.set(person.id, p);
    }
    p.then((url) => live && setSrc(url)).catch(() => undefined);
    return () => {
      live = false;
    };
  }, [api, person.id, plain]);

  const initial = Array.from(person.name.trim())[0] ?? "?";
  return (
    <span className={`avatar avatar-${size}`} aria-hidden="true">
      {src ? <img src={src} alt="" onError={() => setSrc(null)} /> : initial}
    </span>
  );
}
