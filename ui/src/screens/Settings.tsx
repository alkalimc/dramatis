import { useT } from "../i18n";
import { useDispatch, useStore, type SystemPage } from "../store";
import { Dialog } from "../ui/Dialog";
import { Icon, type IconName } from "../ui/Icon";
import { UserAvatar } from "../ui/Avatar";
import { AboutPage } from "./About";
import { EndpointsPage } from "./Endpoints";
import { LanguagePage } from "./Language";
import { QuietSection, QuotaSection } from "./Quota";
import { PrivacyPage } from "./Privacy";
import { SavesPage } from "./Saves";
import { StoragePage } from "./Storage";
import { statusLine, useCurrentSave } from "./Profile";

const PAGES: { page: Exclude<SystemPage, "root">; icon: IconName }[] = [
  { page: "quota", icon: "gauge" },
  { page: "quiet", icon: "bell" },
  { page: "language", icon: "language" },
  { page: "endpoints", icon: "server" },
  { page: "saves", icon: "bookmark" },
  { page: "privacy", icon: "lock" },
  { page: "storage", icon: "archive" },
  { page: "about", icon: "info" },
];

function Row({ icon, label, value, onClick }: { icon: IconName; label: string; value?: string; onClick: () => void }) {
  return (
    <li>
      <button type="button" className="settings-row" onClick={onClick}>
        <Icon name={icon} size={24} />
        <span className="settings-label">{label}</span>
        {value && <span className="settings-value">{value}</span>}
      </button>
    </li>
  );
}

/** Settings as a centred card: a list of sections, each opening its page inside the card. */
export function SettingsCard({ page }: { page: SystemPage }) {
  const t = useT();
  const dispatch = useDispatch();
  const settings = useStore((s) => s.settings);
  const tier = useStore((s) => s.quota?.tier);
  const newer = useStore((s) => s.update?.newer ?? false);
  const language = useStore((s) => s.language);
  const save = useCurrentSave();
  const close = () => dispatch({ type: "drawer", drawer: null });
  const go = (p: SystemPage) => dispatch({ type: "drawer", drawer: { kind: "system", tab: p } });
  const name = settings?.user_name?.trim() || t("user.you");
  const status = statusLine(t, settings?.user_name ?? null);
  const label = (p: Exclude<SystemPage, "root">) => t(`settings.page.${p}`);

  return (
    <Dialog title={t("menu.settings")} onClose={close} className="settings-card" hideTitle>
      <div className="pc-bar pc-bar-edit settings-bar">
        {page !== "root" && (
          <button type="button" className="icon" aria-label={t("chatlist.back")} title={t("chatlist.back")} onClick={() => go("root")}>
            <Icon name="back" size={22} />
          </button>
        )}
        <span className="pc-title" aria-hidden="true">
          {page === "root" ? t("menu.settings") : label(page)}
        </span>
        <button type="button" className="icon" aria-label={t("drawer.close")} title={t("drawer.close")} onClick={close}>
          <Icon name="close" size={20} />
        </button>
      </div>
      {page === "root" ? (
        <div className="settings-root">
          <button type="button" className="settings-head" aria-label={t("menu.profile")} onClick={() => dispatch({ type: "profile", mode: "view" })}>
            <UserAvatar size="l" />
            <span className="left-who">
              <span className="left-name">{name}</span>
              {status && <span className="left-sub">{status}</span>}
            </span>
          </button>
          <ul className="settings-list settings-gap">
            {PAGES.map((p) => (
              <Row key={p.page} icon={p.icon} label={label(p.page)} value={p.page === "quota" && tier ? t(`quota.tier.${tier}`) : p.page === "saves" ? (save ?? undefined) : p.page === "language" ? t(language === "english" ? "lang.english" : "lang.corpus") : p.page === "about" && newer ? t("menu.update") : undefined} onClick={() => go(p.page)} />
            ))}
          </ul>
        </div>
      ) : (
        <div className="settings-page" role="region" aria-label={label(page)}>
          {page === "quota" && <QuotaSection />}
          {page === "quiet" && <QuietSection />}
          {page === "language" && <LanguagePage />}
          {page === "endpoints" && <EndpointsPage />}
          {page === "saves" && <SavesPage />}
          {page === "privacy" && <PrivacyPage />}
          {page === "storage" && <StoragePage />}
          {page === "about" && <AboutPage />}
        </div>
      )}
    </Dialog>
  );
}
