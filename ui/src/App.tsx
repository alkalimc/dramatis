import { useEffect, useMemo, useRef } from "react";
import { api as defaultApi, type Api } from "./api";
import { useT } from "./i18n";
import { createActions, createStore, Provider, subscribe, useActions, useApi, useDispatch, useStore, type Drawer, type Store } from "./store";
import { ErrorBar, InlineError } from "./ui/ErrorBar";
import { unreadOf } from "./lib/chats";
import { Icon, type IconName } from "./ui/Icon";
import { ErrorBoundary } from "./ui/ErrorBoundary";
import { ChatList } from "./screens/ChatList";
import { AskCard } from "./screens/Ask";
import { CaseDrawer, CasesDrawer } from "./screens/Cases";
import { FirstOpen } from "./screens/FirstOpen";
import { GroupDialog } from "./screens/Group";
import { Main } from "./screens/Home";
import { PersonDrawer } from "./screens/Person";
import { RosterDrawer } from "./screens/Roster";
import { SystemDrawer } from "./screens/System";
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
  const group = useStore((s) => s.groupDialog);
  const drawer = useStore((s) => s.drawer);
  const statusError = useStore((s) => s.loadErrors.status);
  const wordingError = useStore((s) => s.loadErrors.wording);
  const full = useStore((s) => s.status?.form === "full" && Boolean(s.hostChannel));
  const actions = useActions();
  usePresence();

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
  const corpus = status.corpus;
  if (corpus.state === "missing" || corpus.state === "unsupported_version" || corpus.state === "missing_requirement") {
    return (
      <main className="refused">
        <p role="alert">
          {corpus.state === "missing"
            ? t("app.corpus.missing")
            : corpus.state === "unsupported_version"
              ? t("app.corpus.unsupported", { version: corpus.format_version })
              : t("app.corpus.requirement", { requirement: corpus.requirement })}
        </p>
      </main>
    );
  }
  if (status.needs_setup) return <FirstOpen />;

  return (
    <div className="app">
      <Rail />
      {full && <ChatList />}
      <div className="stage">
        {wordingError && <p className="banner">{t("app.wording.failed")}</p>}
        <div className="body">
          <Main />
          {drawer && <DrawerPane drawer={drawer} />}
        </div>
      </div>
      <ErrorBar />
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
      if (!document.hidden) void actions.presence();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  }, [actions, full]);
}

const NAV: { key: "roster" | "cases" | "world" | "system"; icon: IconName; drawer: Drawer }[] = [
  { key: "roster", icon: "people", drawer: { kind: "roster" } },
  { key: "cases", icon: "cases", drawer: { kind: "cases" } },
  { key: "world", icon: "world", drawer: { kind: "world" } },
  { key: "system", icon: "system", drawer: { kind: "system", tab: "quota" } },
];

function drawerKey(d: Drawer | null): string | null {
  if (!d) return null;
  if (d.kind === "person") return "roster";
  if (d.kind === "case") return "cases";
  return d.kind;
}

/** The narrow left rail: the main sections as icon and short label, the clock at the foot. */
function Rail() {
  const t = useT();
  const actions = useActions();
  const { mock } = useApi();
  const dispatch = useDispatch();
  const clock = useStore((s) => s.clock);
  const drawer = useStore((s) => s.drawer);
  const unreadChats = useStore((s) => {
    const ids = new Set([...s.channels.map((c) => c.id), ...(s.hostChannel ? [s.hostChannel] : [])]);
    let n = 0;
    for (const id of ids) if (unreadOf(id, s.unread, s.digest) > 0) n += 1;
    return n;
  });
  const current = drawerKey(drawer);

  // The clock is pure data; refresh it on the minute.
  useEffect(() => {
    const timer = setInterval(() => void actions.refreshClock(), 30_000);
    return () => clearInterval(timer);
  }, [actions]);

  const pad = (n: number) => String(n).padStart(2, "0");
  return (
    <header className="rail">
      <h1 className="sr-only">{t("host.name")}</h1>
      <nav aria-label={t("nav.label")}>
        <ul>
          <li>
            <button type="button" className={`rail-item${current === null ? " on" : ""}`} aria-pressed={current === null} onClick={() => dispatch({ type: "drawer", drawer: null })}>
              <Icon name="chats" />
              <span>{t("nav.chats")}</span>
              {unreadChats > 0 && <span className="badge" aria-hidden="true">{unreadChats}</span>}
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
                <Icon name={n.icon} />
                <span>{t(`nav.${n.key}`)}</span>
              </button>
            </li>
          ))}
        </ul>
      </nav>
      <div className="rail-foot">
        {mock && <span className="tag">{t("app.mock")}</span>}
        {clock && (
          <p className="clock" aria-label={t("clock.label")}>
            <time>
              {t("clock.in_world", {
                year: clock.in_world.year,
                month: pad(clock.in_world.month),
                day: pad(clock.in_world.day),
                time: clock.local_time,
              })}
            </time>
          </p>
        )}
      </div>
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
      aria-label={t(`nav.${drawerKey(drawer) as "roster"}`)}
      onKeyDown={(e) => e.key === "Escape" && dispatch({ type: "drawer", drawer: null })}
    >
      <button type="button" className="icon close" aria-label={t("drawer.close")} onClick={() => dispatch({ type: "drawer", drawer: null })}>
        ×
      </button>
      {drawer.kind === "roster" && <RosterDrawer />}
      {drawer.kind === "person" && <PersonDrawer id={drawer.id} />}
      {drawer.kind === "cases" && <CasesDrawer />}
      {drawer.kind === "case" && <CaseDrawer id={drawer.id} />}
      {drawer.kind === "world" && <WorldDrawer />}
      {drawer.kind === "system" && <SystemDrawer tab={drawer.tab} />}
    </aside>
  );
}
