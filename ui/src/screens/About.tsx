import { useState } from "react";
import type { CorpusCredits } from "../api";
import { useT, type T } from "../i18n";
import { useActions, useStore } from "../store";
import { ExternalLink } from "../ui/ExternalLink";

/** "Corpus built by X · licence", from whichever parts the corpus gives; a link when it gives a page. */
function CorpusLine({ t, credits }: { t: T; credits: CorpusCredits }) {
  const { by, licence, url, project } = credits;
  const text = by && licence ? t("about.corpus", { by, licence }) : by ? t("about.corpus.by", { by }) : licence ? t("about.corpus.licence", { licence }) : null;
  if (!text) return null;
  return (
    <p>
      {url ? <ExternalLink href={url}>{text}</ExternalLink> : text}
      {project && (
        <>
          {" "}
          <ExternalLink href={project}>{t("about.corpus.project")}</ExternalLink>
        </>
      )}
    </p>
  );
}

/** The about page of the settings card: who made this, which version, and the update check. */
export function AboutPage() {
  const t = useT();
  const actions = useActions();
  const info = useStore((s) => s.about);
  const update = useStore((s) => s.update);
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);

  const check = async () => {
    setBusy(true);
    setFailed(!(await actions.checkUpdateNow()));
    setBusy(false);
  };
  const version = info?.version ?? update?.current;

  return (
    <section className="about">
      <h3>{t("menu.app")}</h3>
      {version && <p>{t("menu.version", { version })}</p>}
      {info && (
        <>
          <p>{info.author_url ? <ExternalLink href={info.author_url}>{t("about.made_by", { author: info.author })}</ExternalLink> : t("about.made_by", { author: info.author })}</p>
          {info.corpus_credits && <CorpusLine t={t} credits={info.corpus_credits} />}
          <ul className="about-links">
            <li>
              <ExternalLink href={info.repository}>{t("about.repository")}</ExternalLink>
            </li>
            <li>
              <ExternalLink href={`${info.repository}/issues`}>{t("about.issues")}</ExternalLink>
            </li>
          </ul>
          <p className="muted">{t("about.license", { license: info.license })}</p>
        </>
      )}
      <h4 className="section-title">{t("about.update.title")}</h4>
      <p role="status">
        {failed ? (
          t("about.update.failed")
        ) : update?.newer && update.latest ? (
          <>
            {t("about.update.available", { version: update.latest })}{" "}
            <ExternalLink href={update.url ?? `${info?.repository ?? "https://github.com/alkalimc/dramatis"}/releases/latest`}>{t("about.update.view")}</ExternalLink>
          </>
        ) : update?.latest ? (
          t("about.update.current")
        ) : (
          t("about.update.never")
        )}
      </p>
      <p>
        <button type="button" disabled={busy} onClick={() => void check()}>
          {busy ? t("about.update.checking") : t("about.update.check")}
        </button>
      </p>
      <label className="switch">
        <span>{t("about.update.auto")}</span>
        <input type="checkbox" role="switch" checked={update?.auto ?? false} onChange={(e) => void actions.setAutoUpdate(e.target.checked)} />
      </label>
      <p className="muted small">{t("about.update.hint")}</p>
    </section>
  );
}
