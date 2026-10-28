import { render } from "@testing-library/react";
import type { ReactNode } from "react";
import type { Api } from "../api";
import { createMock } from "../api/mock";
import { App } from "../App";
import { createActions, createStore, Provider, type Store } from "../store";

export function mockApi(query = ""): Api {
  const mock = createMock(new URLSearchParams(query), { delay: 0 });
  return { ...mock, mediaUrl: (p) => p, mock: true };
}

/** The whole app on the mock api. */
export function renderApp(query = "", patch?: (api: Api) => Api) {
  const base = mockApi(query);
  const api = patch ? patch(base) : base;
  const store = createStore();
  return { api, store, ...render(<App api={api} store={store} />) };
}

/** One component inside the store provider, with the mock api. */
export function renderWith(node: ReactNode, opts: { query?: string; store?: Store; patch?: (api: Api) => Api } = {}) {
  const base = mockApi(opts.query);
  const api = opts.patch ? opts.patch(base) : base;
  const store = opts.store ?? createStore();
  const actions = createActions(api, store);
  return {
    api,
    store,
    actions,
    ...render(
      <Provider store={store} api={api} actions={actions}>
        {node}
      </Provider>,
    ),
  };
}
