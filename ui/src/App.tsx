import { useEffect, useState } from "react";
import { commands, type AppStatus } from "./api.gen";

// Placeholder shell: proves the generated bindings type-check and reach the engine.
export function App({ load = commands.appStatus }: { load?: typeof commands.appStatus }) {
  const [status, setStatus] = useState<AppStatus | null>(null);

  useEffect(() => {
    let live = true;
    load().then((r) => {
      if (live && r.status === "ok") setStatus(r.data);
    });
    return () => {
      live = false;
    };
  }, [load]);

  return (
    <main>
      <h1>dramatis</h1>
      <p data-testid="form">{status ? status.form : "…"}</p>
    </main>
  );
}
