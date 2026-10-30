import { useEffect, useMemo, useRef, useState } from "react";
import { api as defaultApi, type Api } from "./api";
import { useT } from "./i18n";
import { createActions, createStore, Provider, subscribe, useActions, useDispatch, useStore, type Drawer, type Store } from "./store";
import { ErrorBar, InlineError } from "./ui/ErrorBar";
import { unreadOf } from "./lib/chats";
import { inLibrary } from "./lib/library";
import { readFresh } from "./lib/fresh";
import { isDark } from "./lib/prefs";
import { Icon, type IconName } from "./ui/Icon";
import { ErrorBoundary } from "./ui/ErrorBoundary";
import { ChatList } from "./screens/ChatList";
import { AskCard } from "./screens/Ask";
import { CaseDrawer, CasesDrawer } from "./screens/Cases";
import { FirstOpen } from "./screens/FirstOpen";
import { GroupDialog } from "./screens/Group";
import { Badge } from "./ui/Badge";
import { GroupInfo } from "./screens/GroupInfo";
import { Main } from "./screens/Home";
import { HostCard, HostDrawer, PersonCard, PersonDrawer } from "./screens/Person";
import { LeftDrawer, ProfileCard } from "./screens/Profile";
import { ContactsCard } from "./screens/Contacts";
import { RosterDrawer } from "./screens/Roster";
import { SettingsCard } from "./screens/Settings";
import { ChangedScreen } from "./screens/Changed";
import { UnlockScreen } from "./screens/Unlock";
import { AvatarStep, CorpusSetup, ModelStep, SavePicker, UpdateStep } from "./screens/Welcome";
import { WorldDrawer } from "./screens/World";

/** Wire a store and actions to an api, subscribe to its events, and render the app. */
export function App({ api = defaultApi, store: given }: { api?: Api; store?: Store }) {
  const store = useMemo(() => given ?? createStore(), [given]);
  const actions = useMemo(() => createActions(api, store), [api, store]);

  useEffect(() => {
    let off: (() => void) | undefined;
    let live = true;
    void subscribe(api, store, actions).then((unsub) => {
      if (live) off = unsub;
      else unsub();
    });
    void actions.boot();
    const view = import.meta.env.DEV && api.mock ? new URLSearchParams(location.search).get("view") : null;
    if (view) void import("./dev").then((m) => m.applyDevView(view, store, actions));
    return () => {
      live = false;
      off?.();
    };
  }, [api, store, actions]);

  return (
    <Provider store={store} api={api} actions={actions}>
      <Guarded />
    </Provider>
  );
}

/** A crash inside the app shows the error in the corpus wording once that has loaded. */
function Guarded() {
  const t = useT();
  return (
    <ErrorBoundary t={t}>
      <Shell />
    </ErrorBoundary>
  );
}

function Shell() {
  const t = useT();
  const status = useStore((s) => s.status);
  const ask = useStore((s) => s.ask);
  const personCard = useStore((s) => s.personCard);
  const hostCard = useStore((s) => s.hostCard);
  const update = useStore((s) => s.update);
  const group = useStore((s) => s.groupDialog);
  const drawer = useStore((s) => s.drawer);
  const statusError = useStore((s) => s.loadErrors.status);
  const wordingError = useStore((s) => s.loadErrors.wording);
  const full = useStore((s) => s.status?.form === "full" && Boolean(s.hostChannel));
  const menuOpen = useStore((s) => s.menuOpen);
  const profile = useStore((s) => s.profile);
  const actions = useActions();
  const endpoints = useStore((s) => s.endpoints);
  // Whether this window began on the first-run name form, so the model step can follow it.
  const firstRun = useRef(false);
  if (status?.needs_setup) firstRun.current = true;
  const [avatarDone, setAvatarDone] = useState(false);
  const [modelDone, setModelDone] = useState(false);
  // A step the user went back to, when the usual order would skip it.
  const [forced, setForced] = useState<"name" | "model" | null>(null);
  const chat = endpoints?.roles?.chat;
  const modelMissing = endpoints !== null && (!chat || endpoints.profiles.some((p) => p.profile.name === chat.profile && !p.has_key));
  usePresence();
  useTheme();
  // Native controls and screen readers follow the document's language.
  const locale = t("locale");
  useEffect(() => {
    document.documentElement.lang = locale;
  }, [locale]);

  if (!status) {
    return statusError ? (
      <main className="refused">
        <InlineError error={statusError} onRetry={() => void actions.boot()} />
      </main>
    ) : (
      <p className="loading" role="status">
        {t("app.loading")}
      </p>
    );
  }
  if (status.changed_elsewhere) return <ChangedScreen />;
  if (status.locked) return <UnlockScreen />;
  if (status.corpus.state !== "loaded") return <CorpusSetup corpus={status.corpus} />;
  if (status.save === null) return <SavePicker />;
  // The model step follows the name on a first run, and only then: afterwards the
  // endpoints page in settings is where this lives.
  // Each step can go back one. The first one can only leave the new-save flow, when this
  // window began with a save made by it: back to where the user came from, empty save deleted.
  const fresh = readFresh();
  const leave =
    fresh && fresh.name === status.save
      ? async () => {
          if ((await actions.cancelFresh(fresh)) === "switched") window.location.reload();
        }
      : undefined;
  if (forced === "name") {
    return (
      <FirstOpen
        onBack={leave}
        onDone={() => {
          setForced(null);
          setAvatarDone(false);
        }}
      />
    );
  }
  if (status.needs_setup) return <FirstOpen onBack={leave} />;
  if (firstRun.current && !avatarDone) return <AvatarStep onDone={() => setAvatarDone(true)} onBack={() => setForced("name")} />;
  if (firstRun.current && (forced === "model" || (!modelDone && modelMissing))) {
    return (
      <ModelStep
        onDone={() => {
          setModelDone(true);
          setForced(null);
          void actions.boot();
        }}
        onBack={() => {
          setForced(null);
          setModelDone(false);
          setAvatarDone(false);
        }}
      />
    );
  }

  // The one question about checking for updates, asked once per home, after any first-run steps.
  if (update && !update.asked) return <UpdateStep onBack={firstRun.current ? () => setForced("model") : undefined} />;

  return (
    <div className="app">
      <Rail />
      {full && <ChatList />}
      <div className="stage">
        {wordingError && <p className="banner">{t("app.wording.failed")}</p>}
        <div className="body">
          <Main />
          {drawer && drawer.kind !== "system" && drawer.kind !== "contacts" && <DrawerPane drawer={drawer} />}
        </div>
      </div>
      <ErrorBar />
      {menuOpen && <LeftDrawer />}
      {drawer?.kind === "contacts" && <ContactsCard />}
      {drawer?.kind === "system" && <SettingsCard page={drawer.tab} />}
      {profile && <ProfileCard mode={profile} />}
      {personCard && <PersonCard id={personCard} />}
      {hostCard && <HostCard />}
      {ask && <AskCard draft={ask} />}
      {group && <GroupDialog />}
    </div>
  );
}

/** Coming back to a hidden window is a presence event; the engine decides if it counts. */
function usePresence() {
  const actions = useActions();
  const full = useStore((s) => s.status?.form === "full" && !s.status.needs_setup);
  useEffect(() => {
    if (!full) return;
    const onVisible = () => {
      if (document.hidden) return;
      void actions.markShownRead();
      void actions.presence();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  }, [actions, full]);
}

/** The stored light/dark choice (or the system's) onto the page, following the system live. */
function useTheme() {
  const theme = useStore((s) => s.theme);
  useEffect(() => {
    const root = document.documentElement;
    const apply = () => {
      root.dataset.theme = isDark(theme) ? "dark" : "light";
    };
    apply();
    if (theme !== "system" || typeof matchMedia !== "function") return;
    const query = matchMedia("(prefers-color-scheme: light)");
    query.addEventListener("change", apply);
    return () => query.removeEventListener("change", apply);
  }, [theme]);
}

// Settings live in the hamburger's drawer, not on the rail.
const NAV: { key: "roster" | "cases" | "world"; icon: IconName; drawer: Drawer }[] = [
  { key: "roster", icon: "people", drawer: { kind: "roster" } },
  { key: "cases", icon: "cases", drawer: { kind: "cases" } },
  { key: "world", icon: "world", drawer: { kind: "world" } },
];

function drawerKey(d: Drawer | null): "roster" | "cases" | "world" | "system" | null {
  if (!d || d.kind === "channel" || d.kind === "contacts" || d.kind === "host") return null;
  if (d.kind === "person") return "roster";
  if (d.kind === "case") return "cases";
  return d.kind;
}

/** The narrow left rail: the main sections as icon and short label. */
function Rail() {
  const t = useT();
  const dispatch = useDispatch();
  const drawer = useStore((s) => s.drawer);
  const unreadChats = useStore((s) => {
    let n = 0;
    for (const c of s.channels) if (unreadOf(c, s.unread) > 0) n += 1;
    return n;
  });
  const library = useStore((s) => s.library);
  const libOn = useStore(inLibrary);
  const key = drawerKey(drawer);
  // Settings is a card over the app, not a section: the rail keeps showing the chats.
  const current = libOn ? null : key === "system" ? null : key;

  return (
    <header className="rail">
      <h1 className="sr-only">{t("host.name")}</h1>
      <button type="button" className="rail-menu" aria-label={t("menu.open")} title={t("menu.open")} aria-haspopup="dialog" onClick={() => dispatch({ type: "menu", open: true })}>
        <Icon name="menu" size={26} />
      </button>
      <nav aria-label={t("nav.label")}>
        <ul>
          <li>
            <button
              type="button"
              className={`rail-item${current === null && !libOn ? " on" : ""}`}
              aria-pressed={current === null && !libOn}
              onClick={() => {
                dispatch({ type: "library", on: false });
                dispatch({ type: "drawer", drawer: null });
              }}
            >
              <span className="rail-icon">
                <Icon name="chats" size={30} />
                {unreadChats > 0 && <Badge count={unreadChats} decorative />}
              </span>
              <span className="rail-label">{t("nav.chats")}</span>
            </button>
          </li>
          <li>
            <button type="button" className={`rail-item${libOn ? " on" : ""}`} aria-pressed={libOn} onClick={() => dispatch({ type: "library", on: !library })}>
              <span className="rail-icon">
                <Icon name="library" size={30} />
              </span>
              <span className="rail-label">{t("nav.library")}</span>
            </button>
          </li>
          {NAV.map((n) => (
            <li key={n.key}>
              <button
                type="button"
                className={`rail-item${current === n.key ? " on" : ""}`}
                aria-pressed={current === n.key}
                onClick={() => dispatch({ type: "drawer", drawer: current === n.key ? null : n.drawer })}
              >
                <span className="rail-icon">
                  <Icon name={n.icon} size={30} />
                </span>
                <span className="rail-label">{t(`nav.${n.key}`)}</span>
              </button>
            </li>
          ))}
        </ul>
      </nav>
    </header>
  );
}

function DrawerPane({ drawer }: { drawer: Drawer }) {
  const t = useT();
  const dispatch = useDispatch();
  const ref = useRef<HTMLElement>(null);
  const kind = drawer.kind;
  useEffect(() => {
    ref.current?.focus();
  }, [kind]);
  return (
    <aside
      ref={ref}
      className="drawer"
      tabIndex={-1}
      aria-label={drawer.kind === "channel" ? t("channel.group_info") : drawer.kind === "host" ? t("host.name") : t(`nav.${drawerKey(drawer) as "roster"}`)}
      onKeyDown={(e) => e.key === "Escape" && dispatch({ type: "drawer", drawer: null })}
    >
      <button type="button" className="icon close" aria-label={t("drawer.close")} title={t("drawer.close")} onClick={() => dispatch({ type: "drawer", drawer: null })}>
        <Icon name="close" size={20} />
      </button>
      {drawer.kind === "roster" && <RosterDrawer />}
      {drawer.kind === "person" && <PersonDrawer id={drawer.id} />}
      {drawer.kind === "host" && <HostDrawer />}
      {drawer.kind === "channel" && <GroupInfo id={drawer.id} />}
      {drawer.kind === "cases" && <CasesDrawer />}
      {drawer.kind === "case" && <CaseDrawer id={drawer.id} />}
      {drawer.kind === "world" && <WorldDrawer />}
    </aside>
  );
}
