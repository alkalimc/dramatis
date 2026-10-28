import { useEffect, useId, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { useT, userTitle } from "../i18n";
import { readAvatar } from "../lib/image";
import { isDark } from "../lib/prefs";
import { useActions, useApi, useDispatch, useStore } from "../store";
import { Dialog } from "../ui/Dialog";
import { Icon, type IconName } from "../ui/Icon";
import { UserAvatar } from "../ui/Avatar";
import { birthday, NameAndBirthday } from "./Quota";

/** A picture control: shows the current one, takes a file, and can drop it again. */
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

/** Edit the user's own picture, name and birthday. */
function ProfileDialog({ onClose }: { onClose: () => void }) {
  const t = useT();
  const actions = useActions();
  const id = useId();
  const settings = useStore((s) => s.settings);
  const [name, setName] = useState(settings?.user_name ?? "");
  const [month, setMonth] = useState(settings?.birthday ? String(settings.birthday.month) : "");
  const [day, setDay] = useState(settings?.birthday ? String(settings.birthday.day) : "");
  if (!settings) return null;
  return (
    <Dialog title={t("profile.title")} onClose={onClose}>
      <div className="profile-pic">
        <UserAvatar size="xl" />
        <PictureButtons onPick={async (f) => actions.setUserAvatar(await readAvatar(f))} onClear={() => void actions.setUserAvatar(null)} clearable />
      </div>
      <form
        onSubmit={async (e) => {
          e.preventDefault();
          if (!name.trim()) return;
          if (await actions.saveSettings({ ...settings, user_name: name.trim(), birthday: birthday(month, day) })) onClose();
        }}
      >
        <NameAndBirthday id={id} name={name} setName={setName} month={month} setMonth={setMonth} day={day} setDay={setDay} />
        <p className="muted small">{t("you.preview", { title: userTitle(t, name) })}</p>
        <p className="row end">
          <button type="button" className="ghost" onClick={onClose}>
            {t("group.cancel")}
          </button>
          <button type="submit" disabled={!name.trim()}>
            {t("you.save")}
          </button>
        </p>
      </form>
    </Dialog>
  );
}

function useAppVersion(): string | null {
  const { mock } = useApi();
  const [v, setV] = useState<string | null>(null);
  useEffect(() => {
    if (mock) return;
    void getVersion().then(setV, () => undefined);
  }, [mock]);
  return v;
}

function Row({ icon, label, onClick }: { icon: IconName; label: string; onClick: () => void }) {
  return (
    <li>
      <button type="button" className="drawer-item" onClick={onClick}>
        <Icon name={icon} size={22} />
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
  const archivedCount = useStore((s) => s.channels.filter((c) => c.archived).length);
  const [profile, setProfile] = useState(false);
  const ref = useRef<HTMLElement>(null);
  const version = useAppVersion();
  const close = () => dispatch({ type: "menu", open: false });
  const go = (fn: () => void) => () => {
    close();
    fn();
  };
  const dark = isDark(theme);

  useEffect(() => {
    ref.current?.focus();
  }, []);

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <aside ref={ref} className="left-drawer" tabIndex={-1} aria-label={t("menu.label")} onKeyDown={(e) => e.key === "Escape" && !profile && close()}>
        <header className="left-head">
          <UserAvatar size="l" />
          <p className="left-name">{userTitle(t, name)}</p>
        </header>
        <ul className="drawer-items">
          <Row icon="user" label={t("menu.profile")} onClick={() => setProfile(true)} />
          <Row icon="group" label={t("roster.new_group")} onClick={go(() => dispatch({ type: "groupDialog", open: true }))} />
          <Row icon="people" label={t("menu.contacts")} onClick={go(() => dispatch({ type: "drawer", drawer: { kind: "roster" } }))} />
          <Row icon="bookmark" label={archivedCount > 0 ? t("menu.archived.count", { count: archivedCount }) : t("chatlist.archived")} onClick={go(() => dispatch({ type: "listView", view: "archived" }))} />
          <Row icon="system" label={t("menu.settings")} onClick={go(() => dispatch({ type: "drawer", drawer: { kind: "system", tab: "quota" } }))} />
          <li>
            <label className="drawer-item switch-row">
              <Icon name="moon" size={22} />
              <span>{t("menu.night")}</span>
              <input type="checkbox" role="switch" checked={dark} onChange={(e) => actions.setTheme(e.target.checked ? "dark" : "light")} />
            </label>
          </li>
        </ul>
        <footer className="left-foot muted small">{t("menu.version", { version: version ?? "dev" })}</footer>
      </aside>
      {profile && <ProfileDialog onClose={() => setProfile(false)} />}
    </div>
  );
}
