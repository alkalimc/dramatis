import { useEffect, useId, useRef, useState } from "react";
import { useT, userTitle } from "../i18n";
import { readAvatar } from "../lib/image";
import { isDark } from "../lib/prefs";
import { useActions, useApi, useDispatch, useStore } from "../store";
import { Dialog } from "../ui/Dialog";
import { Icon, type IconName } from "../ui/Icon";
import { Menu } from "../ui/Menu";
import { UserAvatar } from "../ui/Avatar";
import { birthday } from "./Quota";

/** Longest personal bio, in characters; the engine refuses more. */
export const BIO_MAX = 200;

/** A picture control: takes a file, and can drop the current one again. */
export function PictureButtons({ onPick, onClear, clearable }: { onPick: (f: File) => void; onClear: () => void; clearable: boolean }) {
  const t = useT();
  const file = useRef<HTMLInputElement>(null);
  return (
    <div className="row picture-buttons">
      <input
        ref={file}
        type="file"
        accept="image/png,image/jpeg,image/webp,image/gif"
        className="sr-only"
        tabIndex={-1}
        aria-hidden="true"
        onChange={(e) => {
          const f = e.target.files?.[0];
          if (f) onPick(f);
          e.target.value = "";
        }}
      />
      <button type="button" className="ghost" onClick={() => file.current?.click()}>
        <Icon name="camera" size={16} /> {t("picture.change")}
      </button>
      {clearable && (
        <button type="button" className="link" onClick={onClear}>
          {t("picture.remove")}
        </button>
      )}
    </div>
  );
}

/** The name of the save this window has open, once the engine says; `null` until then. */
export function useCurrentSave(): string | null {
  const api = useApi();
  const [name, setName] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    void api.commands.saves().then((r) => {
      if (live && r.status === "ok") setName(r.data.find((s) => s.current)?.name ?? null);
    });
    return () => {
      live = false;
    };
  }, [api]);
  return name;
}

const monthDay = (t: ReturnType<typeof useT>, b: { month: number; day: number }) =>
  new Date(2000, b.month - 1, b.day).toLocaleDateString(t("locale"), { month: "long", day: "numeric" });

/** The line under the name: the in-world title, when the name is set. */
export function statusLine(t: ReturnType<typeof useT>, name: string | null): string | null {
  return name?.trim() ? userTitle(t, name) : null;
}

/** One info row: the icon, the label, and the value at the right edge. */
function InfoRow({ icon, value, label, onClick, open }: { icon: IconName; value: string; label: string; onClick?: () => void; open?: boolean }) {
  const body = (
    <>
      <Icon name={icon} size={24} />
      <span className="info-label">{label}</span>
      <span className="info-value" title={value}>{value}</span>
    </>
  );
  return onClick ? (
    <button type="button" className="info-row" aria-expanded={open} onClick={onClick}>
      {body}
    </button>
  ) : (
    <div className="info-row">{body}</div>
  );
}

/** The user's own card, a centred modal: read it, or change the picture, name and birthday. */
export function ProfileCard({ mode }: { mode: "view" | "edit" }) {
  const t = useT();
  const dispatch = useDispatch();
  const close = () => dispatch({ type: "profile", mode: null });
  const settings = useStore((s) => s.settings);
  const save = useCurrentSave();
  if (!settings) return null;
  const status = statusLine(t, settings.user_name);
  const avatarId = "profile-avatar";
  return (
    <Dialog title={t("profile.title")} onClose={close} className="profile-card" hideTitle>
      {mode === "view" ? (
        <>
          <div className="pc-bar">
            <button type="button" className="icon" aria-label={t("profile.edit")} title={t("profile.edit")} onClick={() => dispatch({ type: "profile", mode: "edit" })}>
              <Icon name="pencil" size={20} />
            </button>
            <button type="button" className="icon" aria-label={t("drawer.close")} title={t("drawer.close")} onClick={close}>
              <Icon name="close" size={20} />
            </button>
          </div>
          <div className="pc-head">
            <UserAvatar size="xl" />
            <p className="pc-name">{settings.user_name?.trim() || t("user.you")}</p>
            {status && <p className="pc-status">{status}</p>}
            {settings.bio && <p className="pc-bio">{settings.bio}</p>}
          </div>
          <div className="pc-rows">
            <InfoRow icon="profile" value={settings.user_name?.trim() || t("user.you")} label={t("profile.name")} />
            <InfoRow icon="calendar" value={settings.birthday ? monthDay(t, settings.birthday) : t("profile.birthday.none")} label={t("profile.birthday")} />
            {save && <InfoRow icon="bookmark" value={save} label={t("profile.save")} />}
          </div>
        </>
      ) : (
        <ProfileEdit avatarId={avatarId} onBack={() => dispatch({ type: "profile", mode: "view" })} onClose={close} />
      )}
    </Dialog>
  );
}

function ProfileEdit({ avatarId, onBack, onClose }: { avatarId: string; onBack: () => void; onClose: () => void }) {
  const t = useT();
  const actions = useActions();
  const id = useId();
  const file = useRef<HTMLInputElement>(null);
  const settings = useStore((s) => s.settings);
  const [name, setName] = useState(settings?.user_name ?? "");
  const [month, setMonth] = useState(settings?.birthday ? String(settings.birthday.month) : "");
  const [day, setDay] = useState(settings?.birthday ? String(settings.birthday.day) : "");
  const [bio, setBio] = useState(settings?.bio ?? "");
  const [editing, setEditing] = useState<"name" | "birthday" | null>(null);
  const [failed, setFailed] = useState(false);
  const [camera, setCamera] = useState<{ x: number; y: number } | null>(null);
  if (!settings) return null;
  const when = birthday(month, day);
  const left = BIO_MAX - Array.from(bio.trim()).length;
  const toggle = (k: "name" | "birthday") => setEditing(editing === k ? null : k);

  const pick = async (f: File) => {
    setFailed(false);
    try {
      await actions.setUserAvatar(await readAvatar(f));
    } catch {
      setFailed(true);
    }
  };

  return (
    <form
      className="pc-edit"
      onSubmit={async (e) => {
        e.preventDefault();
        if (!name.trim() || left < 0) return;
        if (await actions.saveSettings({ ...settings, user_name: name.trim(), birthday: when, bio: bio.trim() || null })) onBack();
      }}
    >
      <div className="pc-bar pc-bar-edit">
        <button type="button" className="icon" aria-label={t("chatlist.back")} title={t("chatlist.back")} onClick={onBack}>
          <Icon name="back" size={22} />
        </button>
        <span className="pc-title" aria-hidden="true">
          {t("profile.edit_title")}
        </span>
        <button type="button" className="icon" aria-label={t("drawer.close")} title={t("drawer.close")} onClick={onClose}>
          <Icon name="close" size={20} />
        </button>
      </div>
      <div className="pc-head">
        <div className="pc-avatar">
          <UserAvatar size="xl" />
          <input
            ref={file}
            id={avatarId}
            type="file"
            accept="image/png,image/jpeg,image/webp,image/gif"
            className="sr-only"
            tabIndex={-1}
            aria-hidden="true"
            onChange={(e) => {
              const f = e.target.files?.[0];
              e.target.value = "";
              if (f) void pick(f);
            }}
          />
          <button
            type="button"
            className="camera-btn"
            aria-label={t("picture.change")}
            title={t("picture.change")}
            aria-haspopup="menu"
            onClick={(e) => {
              const r = e.currentTarget.getBoundingClientRect();
              setCamera({ x: r.left, y: r.bottom + 4 });
            }}
          >
            <Icon name="camera" size={22} />
          </button>
          {camera && (
            <Menu
              at={camera}
              label={t("picture.change")}
              onClose={() => setCamera(null)}
              items={[
                { label: t("picture.change"), icon: "camera", onSelect: () => file.current?.click() },
                { label: t("picture.default"), icon: "reset", onSelect: () => void actions.setUserAvatar(null) },
              ]}
            />
          )}
        </div>
        {failed && (
          <p role="alert" className="pc-failed">
            {t("picture.failed")}
          </p>
        )}
      </div>
      <div className="pc-rows">
        <InfoRow icon="profile" value={name.trim() || t("user.you")} label={t("profile.name")} onClick={() => toggle("name")} open={editing === "name"} />
        {editing === "name" && (
          <div className="pc-field">
            <label htmlFor={`${id}-name`}>{t("you.name")}</label>
            <input id={`${id}-name`} value={name} required autoFocus autoComplete="nickname" onChange={(e) => setName(e.target.value)} />
          </div>
        )}
        <InfoRow icon="calendar" value={when ? monthDay(t, when) : t("profile.birthday.none")} label={t("profile.birthday")} onClick={() => toggle("birthday")} open={editing === "birthday"} />
        {editing === "birthday" && (
          <div className="pc-field pc-birthday">
            <label>
              {t("you.month")} <input type="number" min={1} max={12} inputMode="numeric" value={month} onChange={(e) => setMonth(e.target.value)} />
            </label>
            <label>
              {t("you.day")} <input type="number" min={1} max={31} inputMode="numeric" value={day} onChange={(e) => setDay(e.target.value)} />
            </label>
          </div>
        )}
      </div>
      <div className="bio-head">
        <label htmlFor={`${id}-bio`}>{t("profile.bio")}</label>
        <span className={left < 0 ? "bio-left over" : "bio-left"} aria-label={t("profile.bio.left", { count: left })}>
          {left}
        </span>
      </div>
      <div className="bio-box">
        <textarea id={`${id}-bio`} rows={3} value={bio} onChange={(e) => setBio(e.target.value)} aria-describedby={`${id}-bio-hint`} />
      </div>
      <p id={`${id}-bio-hint`} className="muted small bio-hint">
        {t("profile.bio.hint")}
      </p>
      <p className="muted small pc-note">{t("you.preview", { title: userTitle(t, name) })}</p>
      <p className="row end pc-actions">
        <button type="button" className="ghost" onClick={onBack}>
          {t("group.cancel")}
        </button>
        <button type="submit" disabled={!name.trim() || left < 0}>
          {t("you.save")}
        </button>
      </p>
    </form>
  );
}

function Row({ icon, label, onClick }: { icon: IconName; label: string; onClick: () => void }) {
  return (
    <li>
      <button type="button" className="drawer-item" onClick={onClick}>
        <Icon name={icon} size={24} />
        <span>{label}</span>
      </button>
    </li>
  );
}

/** The hamburger's drawer: who you are, the places you go from here, night mode, the version. */
export function LeftDrawer() {
  const t = useT();
  const dispatch = useDispatch();
  const actions = useActions();
  const name = useStore((s) => s.settings?.user_name ?? null);
  const theme = useStore((s) => s.theme);
  const hasSave = useStore((s) => Boolean(s.status?.save));
  const archivedCount = useStore((s) => s.channels.filter((c) => c.archived).length);
  const ref = useRef<HTMLElement>(null);
  const version = useStore((s) => s.about?.version);
  const newer = useStore((s) => s.update?.newer ?? false);
  const close = () => dispatch({ type: "menu", open: false });
  const go = (fn: () => void) => () => {
    close();
    fn();
  };
  const dark = isDark(theme);
  const sub = statusLine(t, name);

  useEffect(() => {
    ref.current?.focus();
  }, []);

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <aside ref={ref} className="left-drawer" tabIndex={-1} aria-label={t("menu.label")} onKeyDown={(e) => e.key === "Escape" && close()}>
        <button type="button" className="left-head" onClick={() => dispatch({ type: "profile", mode: "view" })}>
          <UserAvatar size="l" />
          <span className="left-who">
            <span className="left-name">{name?.trim() || t("user.you")}</span>
            {sub && <span className="left-sub">{sub}</span>}
          </span>
          <Icon name="chevron" size={20} />
        </button>
        <ul className="drawer-items">
          <Row icon="profile" label={t("menu.profile")} onClick={() => dispatch({ type: "profile", mode: "view" })} />
        </ul>
        <ul className="drawer-items">
          <Row icon="group" label={t("roster.new_group")} onClick={go(() => dispatch({ type: "groupDialog", open: true }))} />
          <Row icon="people" label={t("menu.contacts")} onClick={go(() => dispatch({ type: "drawer", drawer: { kind: "contacts" } }))} />
          <Row icon="archive" label={archivedCount > 0 ? t("menu.archived.count", { count: archivedCount }) : t("chatlist.archived")} onClick={go(() => dispatch({ type: "listView", view: "archived" }))} />
          <Row icon="cases" label={t("nav.cases")} onClick={go(() => dispatch({ type: "drawer", drawer: { kind: "cases" } }))} />
          <Row icon="bookmark" label={t("system.tab.saves")} onClick={go(() => dispatch({ type: "drawer", drawer: { kind: "system", tab: "saves" } }))} />
          <Row icon="system" label={t("menu.settings")} onClick={go(() => dispatch({ type: "drawer", drawer: { kind: "system", tab: "root" } }))} />
          {hasSave && <Row icon="leave" label={t("logout")} onClick={go(() => void actions.logOut())} />}
          <li>
            <label className="drawer-item switch-row">
              <Icon name="moon" size={24} />
              <span>{t("menu.night")}</span>
              <input type="checkbox" role="switch" checked={dark} onChange={(e) => actions.setTheme(e.target.checked ? "dark" : "light")} />
            </label>
          </li>
        </ul>
        <button type="button" className="left-foot" onClick={() => dispatch({ type: "drawer", drawer: { kind: "system", tab: "about" } })}>
          <span>{t("menu.app")}</span>
          {version && <span>{t("menu.version", { version })}</span>}
          {newer && (
            <span className="update-note">
              <span className="update-dot" aria-hidden="true" />
              {t("menu.update")}
            </span>
          )}
        </button>
      </aside>
    </div>
  );
}
