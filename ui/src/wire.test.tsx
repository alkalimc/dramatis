import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { Api, ApiError, Message, Roster } from "./api";
import { makeT, translate, userTitle } from "./i18n";
import { en } from "./i18n/en";
import { hasPersona } from "./lib/persona";
import { createStore, createActions, subscribe } from "./store";
import { MessageView } from "./screens/Message";
import { EndpointsPage } from "./screens/Endpoints";
import { PersonDrawer } from "./screens/Person";
import { RosterList } from "./screens/Roster";
import { errorText, InlineError } from "./ui/ErrorBar";
import { renderApp, renderWith } from "./test/render";

/** An api whose commands are replaced by the given ones. */
const with_ = (over: Record<string, unknown>) => (api: Api): Api => ({ ...api, commands: { ...api.commands, ...over } as Api["commands"] });

describe("wording", () => {
  test("a pack key overrides the default; the rest fall back", () => {
    const t = makeT({ entries: { "host.name": "Aide", "person.noun_plural": "colleagues" } });
    expect(t("host.name")).toBe("Aide");
    expect(t("roster.counts", { total: 3, enabled: 1 })).toBe("All 3 colleagues · enabled 1");
    expect(t("user.you")).toBe(en["user.you"]);
    expect(translate({}, "host.role")).toBe(en["host.role"]);
  });

  test("the header shows the subtitle and the empty roster uses the noun", async () => {
    const store = createStore();
    store.dispatch({ type: "wording", wording: { entries: { "host.role": "Keeper of records", "person.noun_plural": "colleagues" }, scene_marker: { open: "*", close: "*" } } });
    renderWith(<RosterList />, { store });
    expect(screen.getByText("No colleagues on the roster yet.")).toBeDefined();
  });
});

describe("user title", () => {
  const t = makeT({ entries: { "user.title": "Dr. {name}", "user.you": "Friend" } });
  test("name from settings, trimmed", () => expect(userTitle(t, "  Rowan ")).toBe("Dr. Rowan"));
  test("falls back to the pack's word for you when empty", () => {
    expect(userTitle(t, "")).toBe("Friend");
    expect(userTitle(t, "   ")).toBe("Friend");
    expect(userTitle(t, null)).toBe("Friend");
  });

  test("corpus text arrives substituted: placeholders in it are shown as received", () => {
    const m: Message = {
      id: "m1", channel: "c1", author: { kind: "host" }, text: "Hello <user>, welcome to <org>.", actions: [], attachment: null,
      cites: [], confidence: null, relayed: false, task: null, note: null, at: "2030-01-01T00:00:00Z",
    };
    const store = createStore();
    store.dispatch({ type: "settings", settings: { user_name: "Rowan" } as never });
    renderWith(<MessageView message={m} showPlay={false} />, { store });
    expect(screen.getByText(/Hello <user>, welcome to <org>\./)).toBeDefined();
    expect(screen.queryByText(/Rowan/)).toBeNull();
  });
});

describe("errors", () => {
  const codes: ApiError[] = [
    { code: "not_implemented" }, { code: "not_found" }, { code: "invalid", field: "x" }, { code: "disabled", person: "p" },
    { code: "quota_exhausted", release_at: "2030-01-01T00:00:00Z" }, { code: "no_endpoint" },
    { code: "endpoint", detail: "d" }, { code: "keychain", detail: "d" }, { code: "io", detail: "d" },
  ];
  test("every code has its own text, never a raw key", () => {
    const t = makeT({ entries: {} });
    const texts = codes.map((e) => errorText(e, t, {}).text);
    expect(new Set(texts).size).toBe(codes.length);
    for (const x of texts) expect(x).not.toMatch(/^error\./);
    expect(errorText({ code: "unknown", detail: "boom" }, t, {}).detail).toBe("boom");
  });

  test("inline error with retry", async () => {
    const retry = vi.fn();
    renderWith(<InlineError error={{ code: "endpoint", detail: "502" }} onRetry={retry} />);
    expect(screen.getByText("502")).toBeDefined();
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(retry).toHaveBeenCalled();
  });

  test("a failing status shows a calm error and retries", async () => {
    let calls = 0;
    const real = renderApp; // keep the import used
    void real;
    const patch = with_({
      appStatus: () => {
        calls++;
        return Promise.reject(new Error("bridge down"));
      },
    });
    renderApp("", patch);
    expect(await screen.findByText("Something went wrong:")).toBeDefined();
    expect(screen.getByText("bridge down")).toBeDefined();
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(calls).toBeGreaterThan(1));
  });

  test("a failed action lands in the error bar, not an exception", async () => {
    const store = createStore();
    const { actions } = renderWith(<p />, { store, patch: with_({ roster: () => Promise.resolve({ status: "error", error: { code: "io", detail: "disk" } }) }) });
    await act(() => actions.refreshRoster());
    expect(store.getState().loadErrors.roster).toEqual({ code: "io", detail: "disk" });
  });
});

describe("events", () => {
  test("all ten are subscribed and released on unsubscribe", async () => {
    const live = new Set<string>();
    const api = renderWith(<p />).api;
    const names = Object.keys(api.events) as (keyof Api["events"])[];
    expect(names).toHaveLength(10);
    const spy = { ...api, events: Object.fromEntries(names.map((n) => [n, { listen: () => { live.add(n); return Promise.resolve(() => void live.delete(n)); } }])) } as unknown as Api;
    const store = createStore();
    const off = await subscribe(spy, store, createActions(spy, store));
    expect(live.size).toBe(10);
    off();
    expect(live.size).toBe(0);
  });
});

describe("endpoints", () => {
  test("messages wire is offered, with the anthropic preset and cache-write price", async () => {
    const user = userEvent.setup();
    const { actions, store } = renderWith(<EndpointsPage />);
    await act(() => actions.refreshEndpoints());
    expect(store.getState().endpoints?.presets.some((p) => p.name === "anthropic-compatible")).toBe(true);
    await user.click(await screen.findByRole("button", { name: /Add/ }));
    await user.click(screen.getByRole("radio", { name: "anthropic-compatible" }));
    const wire = screen.getByLabelText("Wire API") as HTMLSelectElement;
    expect(wire.value).toBe("messages");
    expect(within(wire).getAllByRole("option")).toHaveLength(3);
    expect(screen.getByDisplayValue("https://api.anthropic.com/v1")).toBeDefined();
  });

  test("host role defaults to the chat model and can be set apart", async () => {
    const user = userEvent.setup();
    const { actions, store } = renderWith(<EndpointsPage />);
    await act(() => actions.refreshEndpoints());
    const host = (await screen.findByLabelText("Host")) as HTMLSelectElement;
    expect(host.value).toBe("");
    expect(within(host).getByRole("option", { name: "Same as chat" })).toBeDefined();
    const [other] = within(host).getAllByRole("option").filter((o) => (o as HTMLOptionElement).value);
    await user.selectOptions(host, (other as HTMLOptionElement).value);
    await waitFor(() => expect(store.getState().endpoints?.roles?.host).toBeTruthy());
    await user.selectOptions(host, "");
    await waitFor(() => expect(store.getState().endpoints?.roles?.host ?? null).toBeNull());
  });

  test("edit form: cache write price shows on the messages wire only; key is never shown", async () => {
    const user = userEvent.setup();
    const { actions } = renderWith(<EndpointsPage />);
    await act(() => actions.refreshEndpoints());
    expect(screen.getAllByText("Stored in keychain").length).toBeGreaterThan(0);
    const card = (await screen.findByRole("heading", { name: "primary" })).closest("li")!;
    await user.click(within(card).getByRole("button", { name: "Edit primary" }));
    await user.click(within(card).getByText("Limits and prices"));
    expect(within(card).queryByLabelText("Cache write price")).toBeNull();
    await user.selectOptions(within(card).getByLabelText("Wire API"), "messages");
    expect(within(card).getByLabelText("Cache write price")).toBeDefined();
  });
});

describe("persons without a persona", () => {
  test("helper: absent counts as available", () => {
    expect(hasPersona({})).toBe(true);
    expect(hasPersona({ has_persona: true })).toBe(true);
    expect(hasPersona({ has_persona: false })).toBe(false);
  });

  test("roster row: muted note and a disabled talk button", () => {
    const store = createStore();
    const roster = {
      total: 1, enabled: 1,
      groups: [{ band: "acquainted", rows: [{ person: { id: "p1", name: "Ann" }, mode: "enabled", recent: null, companions: [], has_persona: false }] }],
    } as unknown as Roster;
    store.dispatch({ type: "roster", roster });
    renderWith(<RosterList />, { store });
    fireOpen();
    expect(screen.getByText("Not available yet")).toBeDefined();
    expect((screen.getByRole("button", { name: "Talk" }) as HTMLButtonElement).disabled).toBe(true);
  });

  test("person page: note and disabled actions", async () => {
    const real = renderWith(<p />).api;
    const page = await real.commands.personPage("none");
    void page;
    const first = (await real.commands.roster());
    const id = first.status === "ok" ? first.data.groups[0].rows[0].person.id : "";
    const patch = with_({
      personPage: async (p: string) => {
        const r = await real.commands.personPage(p);
        return r.status === "ok" ? { status: "ok", data: { ...r.data, has_persona: false } } : r;
      },
    });
    renderWith(<PersonDrawer id={id} />, { patch });
    expect(await screen.findByText("Not available yet")).toBeDefined();
    expect((screen.getByRole("button", { name: "Talk" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Ask something" }) as HTMLButtonElement).disabled).toBe(true);
  });
});

/** Bands other than the closest fold into <details>; open them all so rows are queryable. */
function fireOpen() {
  document.querySelectorAll("details").forEach((d) => (d.open = true));
}
