import { useEffect, useId, useState } from "react";
import type { PersonRef } from "../api";
import { useT, useUserName } from "../i18n";
import { groupTitle } from "../lib/chats";
import { readAvatar } from "../lib/image";
import { useActions, useApi, useDispatch, useStore } from "../store";
import { Avatar, ChannelAvatar, HOST_ID } from "../ui/Avatar";
import { Dialog } from "../ui/Dialog";
import { Icon } from "../ui/Icon";
import { ModeSwitch } from "../ui/ModeSwitch";
import { PersonPicker } from "./PersonPicker";
import { PictureButtons } from "./Profile";

/** The right-hand panel of a group: picture, name, mode, what it is about, who is in it. */
export function GroupInfo({ id }: { id: string }) {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const dispatch = useDispatch();
  const nameId = useId();
  const me = useUserName();
  // A group nobody has spoken in yet is not in the list; the row made when it was opened stands in.
  const group = useStore((s) => s.channels.find((c) => c.id === id) ?? (s.temp?.id === id ? s.temp : undefined));
  const roster = useStore((s) => s.roster);
  const [editing, setEditing] = useState(false);
  const [adding, setAdding] = useState(false);
  const [draft, setDraft] = useState("");

  const rename = useStore((s) => s.drawer?.kind === "channel" && s.drawer.rename === true);
  useEffect(() => {
    setEditing(rename);
    if (rename && group) setDraft(group.name?.trim() ? groupTitle(group, t, me) : "");
    // Only a new group or a new request to rename resets the field.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id, rename]);

  if (!group) return <p className="muted drawer-body">{t("app.loading")}</p>;
  const modeOf = new Map((roster?.groups ?? []).flatMap((g) => g.rows).map((r) => [r.person.id, r.mode] as const));
  const title = groupTitle(group, t, me);
  const named = Boolean(group.name?.trim());

  const save = async () => {
    setEditing(false);
    // Emptied: back to the names of the members.
    await actions.rename(id, draft.trim() || null);
  };
  const put = async (p: PersonRef) => {
    if ((await actions.attempt(api.commands.addMembers(id, [p.id]))) === undefined) return;
    await actions.refreshChannels();
  };
  const takeOut = async (p: PersonRef) => {
    if (!window.confirm(t("group.members.remove.confirm", { name: p.name }))) return;
    if ((await actions.attempt(api.commands.removeMember(id, p.id))) === undefined) return;
    await actions.refreshChannels();
  };
  const remove = async () => {
    if (!window.confirm(t("channel.delete.confirm"))) return;
    if ((await actions.attempt(api.commands.deleteGroup(id))) === undefined) return;
    await actions.refreshChannels();
    await actions.open(null);
  };

  return (
    <div className="drawer-body info-panel">
      <header className="info-head">
        <ChannelAvatar channel={group} size="xl" />
        {editing ? (
          <form
            className="row rename"
            onSubmit={(e) => {
              e.preventDefault();
              void save();
            }}
          >
            <label htmlFor={nameId} className="sr-only">
              {t("group.rename")}
            </label>
            <input id={nameId} value={draft} autoFocus placeholder={groupTitle({ ...group, name: null } as typeof group, t, me)} onChange={(e) => setDraft(e.target.value)} onKeyDown={(e) => e.key === "Escape" && setEditing(false)} />
            <button type="submit">{t("you.save")}</button>
          </form>
        ) : (
          <h2 className="info-name">
            {title}{" "}
            <button
              type="button"
              className="round small"
              aria-label={t("group.rename")}
              title={t("group.rename")}
              onClick={() => {
                setDraft(named ? title : "");
                setEditing(true);
              }}
            >
              <Icon name="pencil" size={16} />
            </button>
          </h2>
        )}
        <p className="muted">{t("group.members.count", { count: group.participants.length })}</p>
        <PictureButtons onPick={async (f) => actions.setChannelAvatar(id, await readAvatar(f))} onClear={() => void actions.setChannelAvatar(id, null)} clearable={Boolean(group.avatar)} />
      </header>

      <div className="info-squares">
        <button type="button" className="square" onClick={() => void actions.archive(id, !group.archived)}>
          <Icon name="archive" size={24} />
          <span>{t(group.archived ? "chatmenu.unarchive" : "chatmenu.archive")}</span>
        </button>
        <button type="button" className="square danger" onClick={remove}>
          <Icon name="trash" size={24} />
          <span>{t("channel.delete")}</span>
        </button>
      </div>

      <section className="info-section">
        <h3>{t("group.mode")}</h3>
        <ModeSwitch kind="group" mode={group.mode} hints="all" label={t("mode.label.group")} onChange={(m) => actions.setMode({ kind: "channel", id }, m)} />
      </section>

      {/* The topic a group was named after is the title already. */}
      {group.topic && group.topic.trim() !== group.name?.trim() && (
        <section className="info-section">
          <h3>{t("group.description")}</h3>
          <p>{group.topic}</p>
        </section>
      )}

      <section className="info-section">
        <h3>{t("group.members")}</h3>
        <ul className="member-list">
          {group.participants.map((p) => (
            <li key={p.id}>
              <button type="button" className="member" onClick={() => dispatch({ type: "drawer", drawer: p.id === HOST_ID ? { kind: "host" } : { kind: "person", id: p.id } })}>
                <Avatar person={p} size="m" />
                <span className="member-text">
                  <span>{p.name}</span>
                  {modeOf.get(p.id) && <span className="muted small">{t(`mode.${modeOf.get(p.id)!}`)}</span>}
                </span>
              </button>
              {group.participants.length > 1 && (
                <button type="button" className="round small" aria-label={t("group.members.remove", { name: p.name })} title={t("group.members.remove", { name: p.name })} onClick={() => void takeOut(p)}>
                  <Icon name="close" size={16} />
                </button>
              )}
            </li>
          ))}
        </ul>
        <button type="button" className="ghost" onClick={() => setAdding(true)}>
          {t("group.members.add")}
        </button>
        {adding && (
          <Dialog title={t("group.members.add.title")} onClose={() => setAdding(false)}>
            <PersonPicker
              label={t("group.members.add.search")}
              exclude={group.participants.map((p) => p.id)}
              withHost
              onPick={(p) => {
                setAdding(false);
                void put(p);
              }}
            />
          </Dialog>
        )}
      </section>

    </div>
  );
}
