import { useEffect, useId, useState } from "react";
import type { Candidate, PersonRef } from "../api";
import { useT } from "../i18n";
import { useActions, useApi, useDispatch, useStore, type AskDraft } from "../store";
import { Dialog } from "../ui/Dialog";
import { PersonPicker } from "./PersonPicker";

/**
 * Ask someone to look into a question. Candidates come from a roster search on the
 * question (no model call) with their reasons; turns stay hidden under "more".
 */
export function AskCard({ draft }: { draft: AskDraft }) {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const dispatch = useDispatch();
  const names = useStore((s) => s.names);
  const qid = useId();
  const tid = useId();
  const [question, setQuestion] = useState(draft.question ?? "");
  const [candidates, setCandidates] = useState<Candidate[] | null>(null);
  const [chosen, setChosen] = useState<string | null>(draft.person ?? null);
  const [other, setOther] = useState<PersonRef | null>(
    draft.person ? { id: draft.person, name: names[draft.person] ?? draft.person } : null,
  );
  const [picking, setPicking] = useState(false);
  const [turns, setTurns] = useState("");
  const [busy, setBusy] = useState(false);

  // Re-search as the question settles; a local retrieval, so a short debounce is enough.
  useEffect(() => {
    const q = question.trim();
    if (!q) {
      setCandidates(null);
      return;
    }
    let live = true;
    const timer = setTimeout(async () => {
      const found = await actions.attempt(api.commands.findPeople(q));
      if (!live) return;
      setCandidates(found ?? []);
      setChosen((cur) => cur ?? found?.[0]?.person.id ?? null);
    }, 250);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [api, actions, question]);

  const close = () => dispatch({ type: "ask", draft: null });

  const submit = async () => {
    if (!chosen || !question.trim()) return;
    setBusy(true);
    const n = Number.parseInt(turns, 10);
    const task = await actions.attempt(
      api.commands.ask(chosen, question.trim(), Number.isFinite(n) && n > 0 ? n : null, draft.parent ?? null),
    );
    setBusy(false);
    if (!task) return;
    close();
    await actions.openDirect(chosen);
  };

  const list = candidates ?? [];
  const extra = other && !list.some((c) => c.person.id === other.id) ? other : null;

  return (
    <Dialog title={t("ask.title")} onClose={close}>
      <form
        className="ask"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <label htmlFor={qid}>{t("ask.question")}</label>
        <textarea id={qid} rows={2} value={question} onChange={(e) => setQuestion(e.target.value)} autoFocus />

        <fieldset className="ask-to">
          <legend>{t("ask.to")}</legend>
          {candidates === null && !extra && <p className="muted">{question.trim() ? t("ask.searching") : t("ask.none")}</p>}
          <ul className="candidates">
            {list.map((c) => (
              <li key={c.person.id}>
                <label>
                  <input type="radio" name="ask-to" checked={chosen === c.person.id} onChange={() => setChosen(c.person.id)} />
                  <span className="name">{c.person.name}</span>
                  <span className="reason">{c.reason}</span>
                </label>
              </li>
            ))}
            {extra && (
              <li>
                <label>
                  <input type="radio" name="ask-to" checked={chosen === extra.id} onChange={() => setChosen(extra.id)} />
                  <span className="name">{extra.name}</span>
                </label>
              </li>
            )}
          </ul>
          {picking ? (
            <PersonPicker
              label={t("ask.other.search")}
              onPick={(p) => {
                setOther(p);
                setChosen(p.id);
                setPicking(false);
              }}
            />
          ) : (
            <button type="button" className="link" onClick={() => setPicking(true)}>
              {t("ask.other")}
            </button>
          )}
        </fieldset>

        <details className="more">
          <summary>{t("ask.more")}</summary>
          <label htmlFor={tid}>{t("ask.turns")}</label>
          <input id={tid} type="number" min={1} inputMode="numeric" value={turns} onChange={(e) => setTurns(e.target.value)} aria-describedby={`${tid}-h`} />
          <p id={`${tid}-h`} className="muted small">
            {t("ask.turns.hint")}
          </p>
        </details>

        <p className="row end">
          <button type="button" className="ghost" onClick={close}>
            {t("ask.cancel")}
          </button>
          <button type="submit" disabled={busy || !chosen || !question.trim()}>
            {t("ask.submit")}
          </button>
        </p>
      </form>
    </Dialog>
  );
}
