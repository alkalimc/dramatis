import { useState } from "react";
import { useT } from "../i18n";
import { useStore } from "../store";
import { Credit } from "../ui/Credit";
import { EndpointEditor, lacksKey } from "./EndpointEditor";
import { NetworkForm } from "./Network";
import { StepBack } from "./Welcome";

/** The last first-run step: the endpoint editor on its own screen, skippable. */
export function ConnectStep({ onDone, onBack }: { onDone: () => void; onBack?: () => void }) {
  const t = useT();
  const view = useStore((s) => s.endpoints);
  const chat = view?.roles?.chat;
  // Behind a proxy the first check fails; the way to set one is on this card, folded away until needed.
  const [network, setNetwork] = useState(false);
  // A chat role whose profile has no key yet: only the key is missing.
  const target = view && chat ? view.profiles.find((p) => p.profile.name === chat.profile && lacksKey(p)) : undefined;

  return (
    <main className="first-open connect">
      <div className="card">
        {onBack && <StepBack onBack={onBack} />}
        <h1>{t("endpoints.setup.title")}</h1>
        <Credit />
        <p>{target ? t("endpoints.setup.key_only", { name: target.profile.name }) : t("endpoints.setup.intro")}</p>
        {view ? (
          <EndpointEditor view={view} profile={target} guided focus={Boolean(target)} onNetwork={() => setNetwork(true)} onDone={onDone} onCancel={onDone} />
        ) : (
          <p className="muted" role="status">
            {t("app.loading")}
          </p>
        )}
        <details className="more" open={network} onToggle={(e) => setNetwork(e.currentTarget.open)}>
          <summary>{t("network.first")}</summary>
          <p className="muted small">{t("network.first.hint")}</p>
          <NetworkForm />
        </details>
      </div>
    </main>
  );
}
