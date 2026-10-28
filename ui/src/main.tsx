import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { ErrorBoundary } from "./ui/ErrorBoundary";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/layout.css";
import "./styles/chat.css";
import "./styles/panels.css";

// The webview's own right-click menu (reload, inspect) has no place in the app; text fields
// keep theirs for cut and paste. Runs on window, after the app's handlers have had their turn.
window.addEventListener("contextmenu", (e) => {
  if (!(e.target instanceof Element) || !e.target.closest("input, textarea, [contenteditable]")) e.preventDefault();
});

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </StrictMode>,
);
