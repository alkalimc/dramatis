import type { ReactNode } from "react";
import type { SaveInfo } from "../api";
import { useT } from "../i18n";
import { when } from "../lib/format";
import { releaseRange } from "../lib/release";
import { Icon } from "../ui/Icon";

/** One labelled fact of a save, as a chip; `dim` marks a value the save does not record. */
function Fact({ label, children, dim = false }: { label: string; children: ReactNode; dim?: boolean }) {
  return (
    <div className="save-fact">
      <dt>{label}</dt>
      <dd>
        <span className={`fact-chip${dim ? " dim" : ""}`}>{children}</span>
      </dd>
    </div>
  );
}

/**
 * What a save's file says about itself, laid out as labelled chips so nobody opens the wrong
 * one: the app version that wrote it, the corpus pack and its version, and the data format.
 * A save with no record of its app shows the releases that wrote its format instead.
 */
export function SaveFacts({ save }: { save: SaveInfo }) {
  const t = useT();
  const { stamp, state } = save;
  if (!stamp) return <p className="muted small save-note">{t(state === "locked" ? "saves.card.locked" : "saves.card.unreadable")}</p>;
  const range = stamp.app ? null : releaseRange(stamp.schema);
  return (
    <>
      <dl className="save-facts">
        <Fact label={t("saves.card.k.version")} dim={!stamp.app && !range}>
          {stamp.app ?? (range ? t("saves.card.v.range", { range }) : t("saves.card.v.unrecorded"))}
        </Fact>
        <Fact label={t("saves.card.k.pack")} dim={!stamp.pack}>
          {stamp.pack ? [stamp.pack, stamp.pack_version].filter(Boolean).join(" ") : t("saves.card.v.unrecorded")}
        </Fact>
        <Fact label={t("saves.card.k.format")}>{stamp.schema}</Fact>
      </dl>
      {(state === "needs_migration" || state === "newer") && (
        <p className="save-note">
          <span className="badge warn-badge">{t(state === "newer" ? "saves.card.newer" : "saves.card.needs_migration")}</span>
        </p>
      )}
    </>
  );
}

export type SaveAsk = "copy" | "rename" | "export";

/**
 * One save: its name and marks, size and time, its versions and pack (`SaveFacts`), and what
 * can be done with it. The startup picker and Settings → Saves both draw it, so the two never differ.
 */
export function SaveCard({ save, picker, onOpen, onAsk, onDelete }: { save: SaveInfo; picker: boolean; onOpen: () => void; onAsk: (kind: SaveAsk) => void; onDelete: () => void }) {
  const t = useT();
  return (
    <li className={save.current ? "save current" : "save"}>
      <div className="save-main">
        <strong>{save.name}</strong> {save.current && <span className="badge">{t("saves.current")}</span>}{" "}
        {save.encrypted && (
          <span className="badge lock-badge">
            <Icon name="lock" size={12} /> {t("saves.encrypted")}
          </span>
        )}
        <div className="muted small">
          {t("saves.size", { kb: save.size_kb })}
          {save.played_at && <> · {t("saves.played", { when: when(save.played_at, t) })}</>}
        </div>
      </div>
      <SaveFacts save={save} />
      <div className="row wrap" role="group" aria-label={t("saves.actions", { name: save.name })}>
        {!save.current && (
          <button type="button" aria-label={picker ? `${t("saves.switch")} ${save.name}` : undefined} onClick={onOpen}>
            {t("saves.switch")}
          </button>
        )}
        {(["copy", "rename", "export"] as const).map((kind) => (
          <button key={kind} type="button" className="ghost" onClick={() => onAsk(kind)}>
            {t(`saves.${kind}`)}
          </button>
        ))}
        {!save.current && (
          <button type="button" className="ghost" onClick={onDelete}>
            {t("saves.delete")}
          </button>
        )}
      </div>
    </li>
  );
}

/** The saves of this home, one card each. */
export function SaveList({ saves, picker, onOpen, onAsk, onDelete }: { saves: SaveInfo[]; picker: boolean; onOpen: (s: SaveInfo) => void; onAsk: (kind: SaveAsk, s: SaveInfo) => void; onDelete: (s: SaveInfo) => void }) {
  return (
    <ul className="save-list">
      {saves.map((s) => (
        <SaveCard key={s.name} save={s} picker={picker} onOpen={() => onOpen(s)} onAsk={(kind) => onAsk(kind, s)} onDelete={() => onDelete(s)} />
      ))}
    </ul>
  );
}
