import { useId, useMemo, useState } from "react";
import type { ForwardTo, Message } from "../api";
import { useT } from "../i18n";
import { useActions, useDispatch, useStore } from "../store";
import { Avatar, HOST_ID } from "../ui/Avatar";
import { Dialog } from "../ui/Dialog";
import { Icon } from "../ui/Icon";

interface Entry {
  key: string;
  name: string;
  to: ForwardTo;
}

/**
 * Where to pass a message on: the favorites, the host, and every person the user can talk
 * to (the one in the current chat too). A person or the host answers it; the favorites
 * only keep it.
 */
export function ForwardDialog({ message }: { message: Message }) {
  const t = useT();
  const actions = useActions();
  const dispatch = useDispatch();
  const id = useId();
  const roster = useStore((s) => s.roster);
  const [q, setQ] = useState("");
  const [busy, setBusy] = useState(false);

  const entries = useMemo<Entry[]>(() => {
    const people = (roster?.groups ?? [])
      .flatMap((g) => g.rows)
      .filter((r) => r.has_persona && r.mode !== "disabled")
      .map((r): Entry => ({ key: `p:${r.person.id}`, name: r.person.name, to: { kind: "person", id: r.person.id } }));
    return [
      { key: "favorites", name: t("favorites.title"), to: { kind: "favorites" } },
      { key: "host", name: t("host.name"), to: { kind: "host" } },
      ...people,
    ];
  }, [roster, t]);
  const needle = q.trim().toLowerCase();
  const shown = needle ? entries.filter((e) => e.name.toLowerCase().includes(needle)) : entries;

  const close = () => dispatch({ type: "forwarding", message: null });
  const go = async (e: Entry) => {
    if (busy) return;
    setBusy(true);
    // On success the action closes this; on failure the error bar says why and the pick stays open.
    await actions.forward(message.id, e.to);
    setBusy(false);
  };

  return (
    <Dialog title={t("forward.title")} onClose={close} className="forward-card">
      <div className="picker">
        <label htmlFor={id} className="sr-only">
          {t("forward.search")}
        </label>
        <input
          id={id}
          type="search"
          autoFocus
          value={q}
          placeholder={t("forward.search")}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && shown[0]) {
              e.preventDefault();
              void go(shown[0]);
            } else if (e.key === "ArrowDown") {
              e.preventDefault();
              (e.currentTarget.closest(".picker")?.querySelector(".forward-list button") as HTMLElement | null)?.focus();
            }
          }}
        />
        {shown.length === 0 && <p className="muted">{t("forward.empty")}</p>}
        <ul className="forward-list">
          {shown.map((e) => (
            <li key={e.key}>
              <button
                type="button"
                className="forward-row"
                disabled={busy}
                onClick={() => void go(e)}
                onKeyDown={(ev) => {
                  const rows = [...(ev.currentTarget.closest("ul")?.querySelectorAll<HTMLElement>("button") ?? [])];
                  const at = rows.indexOf(ev.currentTarget);
                  if (ev.key === "ArrowDown") rows[at + 1]?.focus();
                  else if (ev.key === "ArrowUp") rows[at - 1]?.focus();
                  else return;
                  ev.preventDefault();
                }}
              >
                {e.to.kind === "favorites" ? (
                  <span className="avatar avatar-m avatar-favorites" aria-hidden="true">
                    <Icon name="bookmark" size={20} />
                  </span>
                ) : (
                  <Avatar person={e.to.kind === "host" ? { id: HOST_ID, name: e.name } : { id: e.to.id, name: e.name }} size="m" />
                )}
                <span>{e.name}</span>
              </button>
            </li>
          ))}
        </ul>
      </div>
    </Dialog>
  );
}
