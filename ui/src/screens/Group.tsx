import { useId, useRef, useState } from "react";
import type { Candidate, ImagePart, PersonRef } from "../api";
import { useT } from "../i18n";
import { readAvatar } from "../lib/image";
import { Icon } from "../ui/Icon";
import { useActions, useApi, useDispatch, useStore } from "../store";
import { Dialog } from "../ui/Dialog";
import { modeHintKey } from "../ui/ModeSwitch";

/** Start a group: first its name and picture, then who (by name or by topic) and what about. */
export function GroupDialog() {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const dispatch = useDispatch();
  const roster = useStore((s) => s.roster);
  const whoId = useId();
  const topicId = useId();
  const [members, setMembers] = useState<PersonRef[]>([]);
  const [q, setQ] = useState("");
  const [byTopic, setByTopic] = useState<Candidate[] | null>(null);
  const [topic, setTopic] = useState("");
  const [busy, setBusy] = useState(false);
  const [step, setStep] = useState<"name" | "people">("name");
  const [name, setName] = useState("");
  const [picture, setPicture] = useState<ImagePart | null>(null);
  const nameId = useId();
  const file = useRef<HTMLInputElement>(null);

  const close = () => dispatch({ type: "groupDialog", open: false });
  const add = (p: PersonRef) => setMembers((m) => (m.some((x) => x.id === p.id) ? m : [...m, p]));
  const remove = (id: string) => setMembers((m) => m.filter((x) => x.id !== id));

  const everyone = (roster?.groups ?? []).flatMap((g) => g.rows).filter((r) => r.mode !== "disabled").map((r) => r.person);
  const term = q.trim().toLowerCase();
  const byName = term ? everyone.filter((p) => p.name.toLowerCase().includes(term)) : [];
  const offered = [...byName, ...(byTopic ?? []).map((c) => c.person).filter((p) => !byName.some((x) => x.id === p.id))].filter(
    (p) => !members.some((m) => m.id === p.id),
  );

  const searchTopic = async () => {
    if (!q.trim()) return;
    setByTopic((await actions.attempt(api.commands.findPeople(q.trim()))) ?? []);
  };

  const start = async () => {
    if (members.length === 0) return;
    setBusy(true);
    const channel = await actions.attempt(api.commands.createGroup(members.map((m) => m.id), topic.trim() || null));
    setBusy(false);
    if (!channel) return;
    close();
    // The name and picture were chosen first; they attach to the group once it exists.
    if (name.trim()) await actions.rename(channel, name.trim());
    if (picture) await actions.setChannelAvatar(channel, picture);
    await actions.refreshChannels();
    await actions.open(channel, members);
  };

  if (step === "name") {
    return (
      <Dialog title={t("group.new")} onClose={close} className="group-card" hideTitle>
        <form
          className="group-name-form"
          onSubmit={(e) => {
            e.preventDefault();
            setStep("people");
          }}
        >
          <div className="group-name-row">
            <input
              ref={file}
              type="file"
              accept="image/png,image/jpeg,image/webp,image/gif"
              className="sr-only"
              tabIndex={-1}
              aria-hidden="true"
              onChange={async (e) => {
                const f = e.target.files?.[0];
                e.target.value = "";
                if (f) setPicture(await readAvatar(f));
              }}
            />
            <button type="button" className="camera-big" aria-label={t("picture.change")} title={t("picture.change")} onClick={() => file.current?.click()}>
              {picture ? <img src={`data:${picture.mime};base64,${picture.data_base64}`} alt="" /> : <Icon name="camera" size={34} />}
            </button>
            <div className="underline-field">
              <label htmlFor={nameId}>{t("group.name.label")}</label>
              <input id={nameId} value={name} autoFocus autoComplete="off" onChange={(e) => setName(e.target.value)} />
            </div>
          </div>
          <p className="text-actions">
            <button type="button" className="text-btn" onClick={close}>
              {t("group.cancel")}
            </button>
            <button type="submit" className="text-btn">
              {t("group.next")}
            </button>
          </p>
        </form>
      </Dialog>
    );
  }

  return (
    <Dialog title={t("group.title")} onClose={close}>
      <form
        className="group-form"
        onSubmit={(e) => {
          e.preventDefault();
          void start();
        }}
      >
        <label htmlFor={whoId}>{t("group.who")}</label>
        <ul className="chips" aria-live="polite">
          {members.map((m) => (
            <li key={m.id}>
              <button type="button" className="chip on" aria-label={t("group.who.remove", { name: m.name })} onClick={() => remove(m.id)}>
                {m.name} ×
              </button>
            </li>
          ))}
        </ul>
        <div className="row field-row">
          <input
            id={whoId}
            type="search"
            value={q}
            placeholder={t("group.who.search")}
            onChange={(e) => {
              setQ(e.target.value);
              setByTopic(null);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                void searchTopic();
              }
            }}
          />
          <button type="button" className="ghost" onClick={searchTopic}>
            {t("roster.find.submit")}
          </button>
        </div>
        {offered.length > 0 && (
          <ul className="chips">
            {offered.slice(0, 10).map((p) => (
              <li key={p.id}>
                <button type="button" className="chip" aria-label={t("group.who.add", { name: p.name })} onClick={() => add(p)}>
                  + {p.name}
                </button>
              </li>
            ))}
          </ul>
        )}

        <label htmlFor={topicId}>{t("group.topic")}</label>
        <textarea id={topicId} rows={2} value={topic} onChange={(e) => setTopic(e.target.value)} aria-describedby={`${topicId}-h`} />
        <p id={`${topicId}-h`} className="muted small">
          {t("group.topic.hint")}
        </p>
        <p className="muted small">{t(modeHintKey("frozen", "group"))}</p>

        <p className="row end">
          <button type="button" className="ghost" onClick={() => setStep("name")}>
            {t("chatlist.back")}
          </button>
          <button type="submit" disabled={busy || members.length === 0}>
            {t("group.start")}
          </button>
        </p>
      </form>
    </Dialog>
  );
}
