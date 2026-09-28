import { useT } from "../i18n";
import { useDispatch } from "../store";
import { EndpointsPage } from "./Endpoints";
import { QuotaAndQuiet } from "./Quota";

const TABS = ["quota", "endpoints"] as const;

export function SystemDrawer({ tab }: { tab: (typeof TABS)[number] }) {
  const t = useT();
  const dispatch = useDispatch();
  return (
    <div className="drawer-body system">
      <header className="drawer-head">
        <h2>{t("system.title")}</h2>
      </header>
      <div className="tabs" role="tablist" aria-label={t("system.title")}>
        {TABS.map((k) => (
          <button
            key={k}
            type="button"
            role="tab"
            id={`tab-${k}`}
            aria-selected={tab === k}
            aria-controls={`panel-${k}`}
            tabIndex={tab === k ? 0 : -1}
            onClick={() => dispatch({ type: "drawer", drawer: { kind: "system", tab: k } })}
            onKeyDown={(e) => {
              if (e.key !== "ArrowRight" && e.key !== "ArrowLeft") return;
              const next = TABS[(TABS.indexOf(k) + (e.key === "ArrowRight" ? 1 : TABS.length - 1)) % TABS.length];
              dispatch({ type: "drawer", drawer: { kind: "system", tab: next } });
              document.getElementById(`tab-${next}`)?.focus();
            }}
          >
            {t(`system.tab.${k}`)}
          </button>
        ))}
      </div>
      <div role="tabpanel" id={`panel-${tab}`} aria-labelledby={`tab-${tab}`}>
        {tab === "quota" ? <QuotaAndQuiet /> : <EndpointsPage />}
      </div>
    </div>
  );
}
