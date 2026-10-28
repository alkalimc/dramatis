import { Component, type ReactNode } from "react";
import { makeT, type T } from "../i18n";
import { emptyWording } from "../store/state";

/**
 * Last line of defence: a screen that throws while rendering would leave a blank window.
 * Show the error and a reload button instead. `t` is the corpus wording when the caller
 * has it; the neutral English default otherwise.
 */
export class ErrorBoundary extends Component<{ t?: T; children: ReactNode }, { error: Error | null }> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: unknown) {
    return { error: error instanceof Error ? error : new Error(String(error)) };
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    const t = this.props.t ?? makeT(emptyWording);
    return (
      <main className="refused crash" role="alert">
        <p>{t("app.crash")}</p>
        <code className="raw">{error.message}</code>
        <button type="button" onClick={() => location.reload()}>
          {t("app.reload")}
        </button>
      </main>
    );
  }
}
