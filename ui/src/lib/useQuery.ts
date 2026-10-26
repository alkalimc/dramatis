import { useCallback, useEffect, useState } from "react";
import { settle, type Failure, type Result } from "../api";

/**
 * Screen-local data that is not worth keeping in the store (a person page, a case file):
 * fetched on mount and whenever a dependency (usually a store version counter) changes.
 * A failure is returned, not thrown, so the screen can show it inline with a retry.
 */
export function useQuery<T>(
  run: () => Result<T>,
  deps: unknown[],
): { data: T | undefined; error: Failure | null; loading: boolean; reload: () => void } {
  const [state, setState] = useState<{ data?: T; error: Failure | null; loading: boolean }>({ error: null, loading: true });
  const [tick, setTick] = useState(0);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const stable = useCallback(run, deps);

  useEffect(() => {
    let live = true;
    // Keep what is shown while it reloads; only the flag changes.
    setState((s) => ({ ...s, loading: true }));
    void settle(stable()).then((r) => {
      if (!live) return;
      setState((s) => (r.ok ? { data: r.data, error: null, loading: false } : { data: s.data, error: r.error, loading: false }));
    });
    return () => {
      live = false;
    };
  }, [stable, tick]);

  return { data: state.data, error: state.error, loading: state.loading, reload: () => setTick((n) => n + 1) };
}
