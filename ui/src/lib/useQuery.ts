import { useCallback, useEffect, useState } from "react";
import type { ApiError } from "../api";
import { useActions } from "../store";

type Result<T> = Promise<{ status: "ok"; data: T } | { status: "error"; error: ApiError }>;

/**
 * Screen-local data that is not worth keeping in the store (a person page, a case file):
 * fetched on mount and whenever a dependency (usually a store version counter) changes.
 */
export function useQuery<T>(run: () => Result<T>, deps: unknown[]): { data: T | undefined; reload: () => void } {
  const { attempt } = useActions();
  const [data, setData] = useState<T | undefined>(undefined);
  const [tick, setTick] = useState(0);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const stable = useCallback(run, deps);

  useEffect(() => {
    let live = true;
    void attempt(stable()).then((v) => {
      if (live && v !== undefined) setData(v);
    });
    return () => {
      live = false;
    };
  }, [attempt, stable, tick]);

  return { data, reload: () => setTick((n) => n + 1) };
}
