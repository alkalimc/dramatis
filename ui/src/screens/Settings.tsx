import { useEffect, useRef, type KeyboardEvent } from "react";
import { useT, type T } from "../i18n";
import { isDark } from "../lib/prefs";
import { useDispatch, useStore, type SystemPage } from "../store";
import { Dialog } from "../ui/Dialog";
import { Icon, type IconName } from "../ui/Icon";
import { UserAvatar } from "../ui/Avatar";
import { Row, Section, SettingsPage } from "../ui/SettingsKit";
import { AboutPage } from "./About";
import { EndpointsPage } from "./Endpoints";
import { InputPage, NightPage, comboText } from "./ChatSettings";
import { LanguagePage } from "./Language";
import { NetworkPage } from "./Network";
import { QuietSection, QuotaSection } from "./Quota";
import { PluginsPage } from "./Plugins";
import { PrivacyPage } from "./Privacy";
import { SavesPage } from "./Saves";
import { StoragePage } from "./Storage";
import { statusLine, useCurrentSave } from "./Profile";

export type Sub = Exclude<SystemPage, "root">;

const ICON: Record<Sub, IconName> = {
  chats: "chats",
  night: "moon",
  input: "keyboard",
  quiet: "bell",
  endpoints: "server",
  quota: "gauge",
  saves: "bookmark",
  privacy: "lock",
  plugins: "plugin",
  advanced: "system",
  storage: "archive",
  network: "network",
  language: "language",
  about: "info",
};

/** The main page, in bands: who and what the app talks to, the user's data, the app itself. */
const ROOT: Sub[][] = [
  ["chats", "endpoints", "quota"],
  ["saves", "privacy", "plugins"],
  ["advanced", "language", "about"],
];

/** Pages that are lists of sub-pages. */
export const CHILDREN: Partial<Record<Sub, Sub[]>> = {
  chats: ["night", "input", "quiet"],
  advanced: ["storage", "network"],
};

/** The page one level up from `page`; the main page for the top pages. */
export function parentOf(page: Sub): SystemPage {
  for (const [parent, kids] of Object.entries(CHILDREN)) if (kids?.includes(page)) return parent as Sub;
  return "root";
}

const label = (t: T, p: Sub) => t(`settings.page.${p}`);

/** Keys that are the user typing or choosing in a control, not the card. */
const editing = (el: EventTarget) => el instanceof HTMLElement && (el.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName) && !(el instanceof HTMLInputElement && /^(checkbox|radio|button)$/.test(el.type)));

/**
 * Settings as a centred card of one size: a header (back, title, close) over a scrolling
 * body that is the main list or one page. Escape closes; Backspace and the back arrow go up one level.
 */
export function SettingsCard({ page }: { page: SystemPage }) {
  const t = useT();
  const dispatch = useDispatch();
  const settings = useStore((s) => s.settings);
  const tier = useStore((s) => s.quota?.tier);
  const newer = useStore((s) => s.update?.newer ?? false);
  const language = useStore((s) => s.language);
  const sendKey = useStore((s) => s.sendKey);
  const theme = useStore((s) => s.theme);
  const save = useCurrentSave();
  const close = () => dispatch({ type: "drawer", drawer: null });
  const go = (p: SystemPage) => dispatch({ type: "drawer", drawer: { kind: "system", tab: p } });
  const name = settings?.user_name?.trim() || t("user.you");
  const status = statusLine(t, settings?.user_name ?? null);
  const up = page === "root" ? null : parentOf(page);
  // Moving to another page removes the button that was pressed; focus goes to the page so Escape and Backspace keep working.
  const body = useRef<HTMLDivElement>(null);
  const shown = useRef(page);
  useEffect(() => {
    if (shown.current !== page) body.current?.focus({ preventScroll: true });
    shown.current = page;
  }, [page]);

  const valueOf = (p: Sub): string | undefined => {
    switch (p) {
      case "quota":
        return tier ? t(`quota.tier.${tier}`) : undefined;
      case "saves":
        return save ?? undefined;
      case "language":
        return t(language === "english" ? "lang.english" : "lang.corpus");
      case "input":
        return sendKey === "enter" ? t("chat.send.value.enter") : comboText(t);
      case "night":
        return t(theme === "system" ? "night.system" : isDark(theme) ? "night.dark" : "night.light");
      case "about":
        return newer ? t("menu.update") : undefined;
      default:
        return undefined;
    }
  };
  const nav = (p: Sub) => <Row key={p} icon={ICON[p]} label={label(t, p)} value={valueOf(p)} onClick={() => go(p)} />;

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Backspace" && up !== null && !editing(e.target)) {
      e.preventDefault();
      go(up);
    }
  };

  return (
    <Dialog title={t("menu.settings")} onClose={close} className="settings-card" hideTitle>
      <div className="settings-shell" onKeyDown={onKeyDown}>
      <div className="pc-bar pc-bar-edit settings-bar">
        {up !== null && (
          <button type="button" className="icon" aria-label={t("chatlist.back")} title={t("chatlist.back")} onClick={() => go(up)}>
            <Icon name="back" size={22} />
          </button>
        )}
        <span className="pc-title" aria-hidden="true">
          {page === "root" ? t("menu.settings") : label(t, page)}
        </span>
        <button type="button" className="icon" aria-label={t("drawer.close")} title={t("drawer.close")} onClick={close}>
          <Icon name="close" size={20} />
        </button>
      </div>
      <div className="settings-scroll" ref={body} tabIndex={-1}>
        {page === "root" ? (
          <>
            <button type="button" className="settings-head" aria-label={t("menu.profile")} onClick={() => dispatch({ type: "profile", mode: "view" })}>
              <UserAvatar size="l" />
              <span className="left-who">
                <span className="left-name">{name}</span>
                {status && <span className="left-sub">{status}</span>}
              </span>
            </button>
            {ROOT.map((group, i) => (
              <Section key={i}>
                {i === 0 && <Row icon="profile" label={t("settings.page.account")} onClick={() => dispatch({ type: "profile", mode: "view" })} />}
                {group.map(nav)}
              </Section>
            ))}
          </>
        ) : (
          <SettingsPage label={label(t, page)}>{CHILDREN[page] ? <Section>{CHILDREN[page]!.map(nav)}</Section> : <PageBody page={page} />}</SettingsPage>
        )}
      </div>
      </div>
    </Dialog>
  );
}

/** The page behind a leaf of the tree. */
function PageBody({ page }: { page: Sub }) {
  switch (page) {
    case "night":
      return <NightPage />;
    case "input":
      return <InputPage />;
    case "quiet":
      return <QuietSection />;
    case "quota":
      return <QuotaSection />;
    case "endpoints":
      return <EndpointsPage />;
    case "saves":
      return <SavesPage />;
    case "privacy":
      return <PrivacyPage />;
    case "plugins":
      return <PluginsPage />;
    case "storage":
      return <StoragePage />;
    case "network":
      return <NetworkPage />;
    case "language":
      return <LanguagePage />;
    case "about":
      return <AboutPage />;
    default:
      return null;
  }
}
