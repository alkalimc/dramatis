import { useT } from "../i18n";
import { useActions, useDispatch, useStore } from "../store";
import { ModeSwitch } from "../ui/ModeSwitch";

/** The first-open recommendation card, attached to the host's introduction. */
export function ColdstartCard() {
  const t = useT();
  const actions = useActions();
  const dispatch = useDispatch();
  const picks = useStore((s) => s.coldstart);
  if (picks.length === 0) return null;
  return (
    <section className="card coldstart" aria-labelledby="coldstart-title">
      <h2 id="coldstart-title">{t("coldstart.title")}</h2>
      <ul className="candidates">
        {picks.map((c) => (
          <li key={c.person.id}>
            <button type="button" className="link name" onClick={() => actions.openDirect(c.person.id)}>
              {c.person.name}
            </button>
            <span className="reason">{c.reason}</span>
            <ModeSwitch
              mode={c.mode}
              hints="none"
              label={t("mode.label", { name: c.person.name })}
              onChange={(m) => actions.setMode({ kind: "person", id: c.person.id }, m)}
            />
          </li>
        ))}
      </ul>
      <p className="muted">{t("coldstart.frozen")}</p>
      <p className="row">
        <span className="muted">{t("coldstart.roster")}</span>
        <button type="button" onClick={() => dispatch({ type: "drawer", drawer: { kind: "roster" } })}>
          {t("coldstart.go_roster")}
        </button>
      </p>
    </section>
  );
}
