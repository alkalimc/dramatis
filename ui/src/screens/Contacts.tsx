import { useState } from "react";
import { useT } from "../i18n";
import { activityText } from "../lib/format";
import { hasPersona } from "../lib/persona";
import { useActions, useDispatch, useStore } from "../store";
import { Dialog } from "../ui/Dialog";
import { PeopleSearch, personEntry } from "./PeopleList";

/** Everyone on the roster in one searchable list; a click opens the conversation with them. */
export function ContactsCard() {
  const t = useT();
  const dispatch = useDispatch();
  const actions = useActions();
  const roster = useStore((s) => s.roster);
  const [q, setQ] = useState("");
  const close = () => dispatch({ type: "drawer", drawer: null });
  const term = q.trim().toLowerCase();
  const rows = (roster?.groups ?? []).flatMap((g) => g.rows).filter((r) => r.mode !== "disabled" && (!term || r.person.name.toLowerCase().includes(term)));

  return (
    <Dialog title={t("menu.contacts")} onClose={close} className="contacts-card" hideTitle>
      <h3 className="card-title" aria-hidden="true">
        {t("menu.contacts")}
      </h3>
      <PeopleSearch
        value={q}
        onChange={setQ}
        placeholder={t("chatlist.search")}
        listLabel={t("menu.contacts")}
        entries={rows.map((r) => personEntry(r.person, { secondary: r.recent ? activityText(r.recent, t) : t(`mode.${r.mode}`), disabled: !hasPersona(r) }))}
        empty={term ? t("chatlist.search.empty") : t("roster.empty")}
        size="l"
        autoFocus
        onPick={(e) => void actions.openDirect(e.key)}
      />
      <p className="text-actions contacts-foot">
        <button type="button" className="text-btn" onClick={close}>
          {t("drawer.close")}
        </button>
      </p>
    </Dialog>
  );
}
