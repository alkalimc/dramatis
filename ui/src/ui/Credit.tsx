import { useT } from "../i18n";
import { useStore } from "../store";
import { ExternalLink } from "./ExternalLink";

const NAME = "dramatis";
const PROJECT = "https://github.com/alkalimc/dramatis";
const AUTHOR = { name: "alkali", url: "https://github.com/alkalimc" };

/** "dramatis · by alkali": the project and its author, each a link; quiet text under a title. */
export function Credit() {
  const t = useT();
  const about = useStore((s) => s.about);
  return (
    <p className="credit">
      <ExternalLink href={about?.repository ?? PROJECT}>{NAME}</ExternalLink> · {t("credit.by")}{" "}
      <ExternalLink href={about?.author_url ?? AUTHOR.url}>{about?.author ?? AUTHOR.name}</ExternalLink>
    </p>
  );
}
