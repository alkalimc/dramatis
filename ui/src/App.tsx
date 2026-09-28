import { useEffect, useMemo, useRef } from "react";
import { api as defaultApi, type Api } from "./api";
import { useT } from "./i18n";
import { createActions, createStore, Provider, subscribe, useActions, useApi, useDispatch, useStore, type Drawer, type Store } from "./store";
import { ErrorBar } from "./ui/ErrorBar";
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
      <Shell />
    </Provider>
  );
}

function Shell() {
  const t = useT();
  const status = useStore((s) => s.status);
  const ask = useStore((s) => s.ask);
  const group = useStore((s) => s.groupDialog);
  const drawer = useStore((s) => s.drawer);
  usePresence();

  if (!status) return <p className="loading">{t("app.loading")}</p>;
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
      <Header />
      {corpus.encoder_mismatch && <p className="banner">{t("app.corpus.encoder")}</p>}
      <div className="body">
        <Main />
        {drawer && <DrawerPane drawer={drawer} />}
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

const NAV: { key: "roster" | "cases" | "world" | "system"; drawer: Drawer }[] = [
  { key: "roster", drawer: { kind: "roster" } },
  { key: "cases", drawer: { kind: "cases" } },
  { key: "world", drawer: { kind: "world" } },
  { key: "system", drawer: { kind: "system", tab: "quota" } },
];

function drawerKey(d: Drawer | null): string | null {
  if (!d) return null;
  if (d.kind === "person") return "roster";
  if (d.kind === "case") return "cases";
  return d.kind;
}

function Header() {
  const t = useT();
  const actions = useActions();
  const { mock } = useApi();
  const dispatch = useDispatch();
  const clock = useStore((s) => s.clock);
  const drawer = useStore((s) => s.drawer);
  const current = drawerKey(drawer);

  // The clock is pure data; refresh it on the minute.
  useEffect(() => {
    const timer = setInterval(() => void actions.refreshClock(), 30_000);
    return () => clearInterval(timer);
  }, [actions]);

  const pad = (n: number) => String(n).padStart(2, "0");
  return (
    <header className="app-header">
      <h1>
        <button type="button" className="link title" onClick={() => actions.open(null)}>
          {t("host.name")}
        </button>
      </h1>
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
      <nav aria-label={t("nav.label")}>
        <ul>
          <li>
            <button type="button" className="nav" aria-label={t("nav.home")} onClick={() => actions.open(null)}>
              ⌂
            </button>
          </li>
          {NAV.map((n) => (
            <li key={n.key}>
              <button
                type="button"
                className={`nav${current === n.key ? " on" : ""}`}
                aria-pressed={current === n.key}
                onClick={() => dispatch({ type: "drawer", drawer: current === n.key ? null : n.drawer })}
              >
                {t(`nav.${n.key}`)}
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
