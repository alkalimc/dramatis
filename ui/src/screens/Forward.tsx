import { useMemo, useState } from "react";
import type { ForwardTo, Message } from "../api";
import { useT } from "../i18n";
import { useActions, useDispatch, useStore } from "../store";
import { HOST_ID } from "../ui/Avatar";
import { Dialog } from "../ui/Dialog";
import { Icon } from "../ui/Icon";
import { PeopleSearch, type Entry as Row } from "./PeopleList";

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

  const rows: Row[] = shown.map((e) => ({
    key: e.key,
    name: e.name,
    value: e.to.kind === "person" ? { id: e.to.id, name: e.name } : e.to.kind === "host" ? { id: HOST_ID, name: e.name } : undefined,
    avatar:
      e.to.kind === "favorites" ? (
        <span className="avatar avatar-m avatar-favorites" aria-hidden="true">
          <Icon name="bookmark" size={20} />
        </span>
      ) : undefined,
    disabled: busy,
  }));

  return (
    <Dialog title={t("forward.title")} onClose={close} className="forward-card">
      <PeopleSearch
        value={q}
        onChange={setQ}
        placeholder={t("forward.search")}
        listLabel={t("forward.title")}
        entries={rows}
        empty={t("forward.empty")}
        enter="first"
        autoFocus
        onPick={(r) => {
          const e = shown.find((x) => x.key === r.key);
          if (e) void go(e);
        }}
      />
    </Dialog>
  );
}
