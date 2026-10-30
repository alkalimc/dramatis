import { useId, useState } from "react";
import { useT } from "../i18n";
import { activityText } from "../lib/format";
import { hasPersona } from "../lib/persona";
import { useActions, useDispatch, useStore } from "../store";
import { Avatar } from "../ui/Avatar";
import { Dialog } from "../ui/Dialog";
import { Icon } from "../ui/Icon";

/** Everyone on the roster in one searchable list; a click opens the conversation with them. */
export function ContactsCard() {
  const t = useT();
  const dispatch = useDispatch();
  const actions = useActions();
  const id = useId();
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
      <div className="contacts-search">
        <Icon name="search" size={20} />
        <label htmlFor={id} className="sr-only">
          {t("chatlist.search")}
        </label>
        <input id={id} type="search" autoFocus value={q} placeholder={t("chatlist.search")} onChange={(e) => setQ(e.target.value)} />
      </div>
      <div className="contacts-list">
        {rows.length === 0 ? (
          <p className="muted pad">{term ? t("chatlist.search.empty") : t("roster.empty")}</p>
        ) : (
          <ul>
            {rows.map((r) => (
              <li key={r.person.id}>
                <button type="button" className="contact-row" disabled={!hasPersona(r)} onClick={() => void actions.openDirect(r.person.id)}>
                  <Avatar person={r.person} size="l" />
                  <span className="chat-main">
                    <span className="chat-name">{r.person.name}</span>
                    <span className="chat-preview muted">{r.recent ? activityText(r.recent, t) : t(`mode.${r.mode}`)}</span>
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
      <p className="text-actions contacts-foot">
        <button type="button" className="text-btn" onClick={close}>
          {t("drawer.close")}
        </button>
      </p>
    </Dialog>
  );
}
