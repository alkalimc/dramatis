import { useState } from "react";
import type { CorpusCredits } from "../api";
import { useT, type T } from "../i18n";
import { useActions, useStore } from "../store";
import { ExternalLink } from "../ui/ExternalLink";
import { Body, Note, Row, Section } from "../ui/SettingsKit";

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
    <>
      <Section title={t("menu.app")}>
        {version && <Row kind="info" icon="info" label={t("menu.version", { version })} />}
        {info && (
          <>
            <Body className="about">
              <p>{info.author_url ? <ExternalLink href={info.author_url}>{t("about.made_by", { author: info.author })}</ExternalLink> : t("about.made_by", { author: info.author })}</p>
              {info.corpus_credits && <CorpusLine t={t} credits={info.corpus_credits} />}
              {info.corpus_version && <p>{t("about.corpus.version", { version: info.corpus_version })}</p>}
              <ul className="about-links">
                <li>
                  <ExternalLink href={info.repository}>{t("about.repository")}</ExternalLink>
                </li>
                <li>
                  <ExternalLink href={`${info.repository}/issues`}>{t("about.issues")}</ExternalLink>
                </li>
              </ul>
            </Body>
            <Note>{t("about.license", { license: info.license })}</Note>
          </>
        )}
      </Section>
      <Section title={t("about.update.title")}>
        <Body>
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
        </Body>
        <Row kind="button" icon="reset" disabled={busy} busy={busy} label={busy ? t("about.update.checking") : t("about.update.check")} onClick={() => void check()} />
        <Row kind="toggle" label={t("about.update.auto")} checked={update?.auto ?? false} onChange={(on) => void actions.setAutoUpdate(on)} />
        <Note>{t("about.update.hint")}</Note>
      </Section>
    </>
  );
}
