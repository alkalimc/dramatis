import { useT } from "../i18n";
import { useStore } from "../store";
import { Credit } from "../ui/Credit";
import { EndpointEditor, lacksKey } from "./EndpointEditor";
import { StepBack } from "./Welcome";

/** The last first-run step: the endpoint editor on its own screen, skippable. */
export function ConnectStep({ onDone, onBack }: { onDone: () => void; onBack?: () => void }) {
  const t = useT();
  const view = useStore((s) => s.endpoints);
  const chat = view?.roles?.chat;
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
          <EndpointEditor view={view} profile={target} guided focus={Boolean(target)} onDone={onDone} onCancel={onDone} />
        ) : (
          <p className="muted" role="status">
            {t("app.loading")}
          </p>
        )}
      </div>
    </main>
  );
}
