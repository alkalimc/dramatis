import { createContext, createElement, useContext, useSyncExternalStore, type ReactNode } from "react";
import type { Api } from "../api";
import type { Actions } from "./actions";
import { reduce } from "./reducer";
import { initialState, type Action, type State } from "./state";

export type { Action, AskDraft, Drawer, State, Stream } from "./state";
export { initialState } from "./state";
export { reduce } from "./reducer";
export { createActions, subscribe, type Actions } from "./actions";

/** The one external store: fed by api events and command results, read through hooks. */
export interface Store {
  getState(): State;
  dispatch(action: Action): void;
  subscribe(listener: () => void): () => void;
}

export function createStore(initial: State = initialState()): Store {
  let state = initial;
  const listeners = new Set<() => void>();
  return {
    getState: () => state,
    dispatch(action) {
      const next = reduce(state, action);
      if (next === state) return;
      state = next;
      for (const l of listeners) l();
    },
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

interface Ctx {
  store: Store;
  api: Api;
  actions: Actions;
}

const Context = createContext<Ctx | null>(null);

export function Provider({ store, api, actions, children }: Ctx & { children: ReactNode }) {
  return createElement(Context.Provider, { value: { store, api, actions } }, children);
}

function useCtx(): Ctx {
  const ctx = useContext(Context);
  if (!ctx) throw new Error("store Provider missing");
  return ctx;
}

/** Select a slice. Selectors must return stored references, not fresh objects. */
export function useStore<T>(select: (s: State) => T): T {
  const { store } = useCtx();
  return useSyncExternalStore(
    store.subscribe,
    () => select(store.getState()),
    () => select(store.getState()),
  );
}

export function useDispatch(): Store["dispatch"] {
  return useCtx().store.dispatch;
}

export function useApi(): Api {
  return useCtx().api;
}

export function useActions(): Actions {
  return useCtx().actions;
}
