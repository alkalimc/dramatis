import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { createActions } from "../store";
import { clearFresh, markFresh, readFresh } from "../lib/fresh";
import { renderApp, renderWith } from "../test/render";
import { SavePicker } from "./Welcome";

test("no save open: the picker leads, naming the folder", async () => {
  renderApp("nosave=1");
  expect(await screen.findByRole("heading", { name: "Choose a save" })).toBeDefined();
  expect(screen.getByText("/Users/demo/.dramatis/saves")).toBeDefined();
  expect(screen.getByRole("button", { name: "Open default" })).toBeDefined();
});

test("opening a save from the picker switches to it and hands over", async () => {
  const user = userEvent.setup();
  const onSwitched = vi.fn();
  const { api } = renderWith(<SavePicker onSwitched={onSwitched} />, { query: "nosave=1" });
  await user.click(await screen.findByRole("button", { name: "Open second try" }));
  await waitFor(() => expect(onSwitched).toHaveBeenCalledOnce());
  const r = await api.commands.appStatus();
  expect(r.status === "ok" && r.data.save).toBe("second try");
});

test("a last save that is gone is named, and a home with no saves offers the first", async () => {
  renderApp("nosave=1&saves=0&missing=old+story");
  expect(await screen.findByRole("heading", { name: "Create your first save" })).toBeDefined();
  expect(screen.getByText(/The save you used last, old story, is not there any more/)).toBeDefined();
});

test("creating a save from the picker opens it", async () => {
  const user = userEvent.setup();
  const onSwitched = vi.fn();
  const { api } = renderWith(<SavePicker onSwitched={onSwitched} />, { query: "nosave=1&saves=0" });
  await user.type(await screen.findByLabelText("Name of the new save"), "fresh");
  await user.click(screen.getByRole("button", { name: "Create and open" }));
  await waitFor(() => expect(onSwitched).toHaveBeenCalledOnce());
  const r = await api.commands.appStatus();
  expect(r.status === "ok" && r.data.save).toBe("fresh");
});

test("log out from the menu leads to the picker, and nothing of the old save stays on screen", async () => {
  const user = userEvent.setup();
  renderApp();
  await screen.findByRole("heading", { name: "Who's waiting" });
  await user.click(screen.getByRole("button", { name: "Menu" }));
  await user.click(within(await screen.findByRole("complementary", { name: "Menu" })).getByRole("button", { name: "Log out of this save" }));
  expect(await screen.findByRole("heading", { name: "Choose a save" })).toBeDefined();
  expect(screen.queryByRole("heading", { name: "Who's waiting" })).toBeNull();
});

test("the unlock screen has a log out link that leads to the picker", async () => {
  const user = userEvent.setup();
  renderApp("locked=1");
  await user.click(await screen.findByRole("button", { name: "Log out" }));
  expect(await screen.findByRole("heading", { name: "Choose a save" })).toBeDefined();
});

test("no corpus: a bad path says why, a good one goes on", async () => {
  const user = userEvent.setup();
  renderApp("corpus=missing");
  const field = await screen.findByLabelText("Path of the .folio file");
  await user.type(field, "/tmp/readme.txt");
  await user.click(screen.getByRole("button", { name: "Use this corpus" }));
  expect(await screen.findByText("That file is not a corpus this app can read.")).toBeDefined();
  await user.clear(field);
  await user.type(field, "/tmp/missing.folio");
  await user.click(screen.getByRole("button", { name: "Use this corpus" }));
  expect(await screen.findByText("There is no file at that path.")).toBeDefined();
  await user.clear(field);
  await user.type(field, "/tmp/newer.folio");
  await user.click(screen.getByRole("button", { name: "Use this corpus" }));
  expect(await screen.findByText("That corpus was written for another version of the app.")).toBeDefined();
  await user.clear(field);
  await user.type(field, "/tmp/demo.folio");
  await user.click(screen.getByRole("button", { name: "Use this corpus" }));
  expect(await screen.findByRole("heading", { name: "Who's waiting" })).toBeDefined();
});

test("a file dialog fills the path; without one the field says to type it", async () => {
  const user = userEvent.setup();
  const { unmount } = renderApp("corpus=missing&dialog=1");
  await user.click(await screen.findByRole("button", { name: "Choose file" }));
  expect(await screen.findByRole("heading", { name: "Who's waiting" })).toBeDefined();
  unmount();

  renderApp("corpus=missing");
  await user.click(await screen.findByRole("button", { name: "Choose file" }));
  expect(await screen.findByText("No file dialog is available here. Type the path instead.")).toBeDefined();
});

/** Through the name and picture steps to the endpoint step. */
async function toConnect(user: ReturnType<typeof userEvent.setup>) {
  await user.type(await screen.findByLabelText("What should they call you?"), "Rowan");
  await user.click(screen.getByRole("button", { name: "Continue" }));
  await user.click(await screen.findByRole("button", { name: "Keep the default" }));
  expect(await screen.findByRole("heading", { name: "Connect a language model" })).toBeDefined();
}

test("first run ends with the endpoint step: preset, key, check, model, then the host speaks first", async () => {
  const user = userEvent.setup();
  renderApp("first=1&form=library");
  await toConnect(user);

  await user.click(screen.getByRole("radio", { name: "deepseek" }));
  expect((screen.getByLabelText("Base URL") as HTMLInputElement).value).toBe("https://api.deepseek.com");
  // No model is asked for before the endpoint has been checked, and a missing key is named.
  expect(screen.queryByLabelText("Model for chat")).toBeNull();
  await user.click(screen.getByRole("button", { name: "Check connection" }));
  expect(await screen.findByText("This address needs a key. Enter it.")).toBeDefined();

  await user.type(screen.getByLabelText("API key"), "sk-test");
  await user.click(screen.getByRole("button", { name: "Check connection" }));
  const model = (await screen.findByLabelText("Model for chat")) as HTMLSelectElement;
  expect(screen.getByText("Models found: 2")).toBeDefined();
  expect(model.value).toBe("deepseek-chat");
  expect(screen.queryByDisplayValue("sk-test")).toBeNull();
  await user.selectOptions(model, "deepseek-reasoner");
  await user.click(screen.getByRole("button", { name: "Save and continue" }));
  await user.click(await screen.findByRole("button", { name: "Not now" }));
  expect(await within(await screen.findByRole("log")).findByText(/Hello, Rowan\./)).toBeDefined();
});

test("a key the endpoint refuses and an address that does not answer are told apart; a model can be typed", async () => {
  const user = userEvent.setup();
  renderApp("first=1&form=library");
  await toConnect(user);

  await user.click(screen.getByRole("radio", { name: "deepseek" }));
  await user.type(screen.getByLabelText("API key"), "bad-key");
  await user.click(screen.getByRole("button", { name: "Check connection" }));
  expect(await screen.findByText(/401 Unauthorized/)).toBeDefined();
  expect(screen.getByText(/did not accept the key/)).toBeDefined();
  // The key was stored and is not shown again; replacing it is one click away.
  expect(screen.getByText("Stored in keychain")).toBeDefined();
  await user.click(screen.getByRole("button", { name: "Replace key" }));
  await user.type(screen.getByLabelText("API key"), "sk-good");
  await user.click(screen.getByRole("button", { name: "Refresh models" }));
  expect(await screen.findByText("Models found: 2")).toBeDefined();

  await user.click(screen.getByRole("radio", { name: "Custom" }));
  await user.type(screen.getByLabelText("Base URL"), "https://unreachable.invalid/v1");
  await user.type(screen.getByLabelText("API key"), "sk-x");
  await user.click(screen.getByRole("button", { name: "Check connection" }));
  expect(await screen.findByText(/No usable answer from the address/)).toBeDefined();
  await user.type(screen.getByLabelText("Model for chat"), "my-model");
  await user.click(screen.getByRole("button", { name: "Save and continue" }));
  await user.click(await screen.findByRole("button", { name: "Not now" }));
  expect(await within(await screen.findByRole("log")).findByText(/Hello, Rowan\./)).toBeDefined();
});

test("a local server needs no key; the endpoint step can be skipped", async () => {
  const user = userEvent.setup();
  renderApp("first=1&form=library");
  await toConnect(user);
  await user.click(screen.getByRole("radio", { name: "local" }));
  expect((screen.getByLabelText("Base URL") as HTMLInputElement).value).toBe("http://127.0.0.1:8080/v1");
  expect(screen.getByText(/needs no key/)).toBeDefined();
  await user.click(screen.getByRole("button", { name: "Check connection" }));
  expect(await screen.findByDisplayValue("local-model")).toBeDefined();
  await user.click(screen.getByRole("button", { name: "Skip for now" }));
  await user.click(await screen.findByRole("button", { name: "Not now" }));
  expect(await screen.findByText(/Not connected\./)).toBeDefined();
});

test("first run asks for a picture after the name and before the model, and it can be skipped", async () => {
  const user = userEvent.setup();
  const set: unknown[] = [];
  renderApp("first=1&form=library", (api) => ({
    ...api,
    commands: {
      ...api.commands,
      setUserAvatar: (image) => {
        set.push(image);
        return api.commands.setUserAvatar(image);
      },
    },
  }));
  await user.type(await screen.findByLabelText("What should they call you?"), "Rowan");
  await user.click(screen.getByRole("button", { name: "Continue" }));
  expect(await screen.findByRole("heading", { name: "Your picture" })).toBeDefined();
  expect(screen.queryByRole("heading", { name: "Connect a language model" })).toBeNull();
  await user.click(screen.getByRole("button", { name: "Keep the default" }));
  expect(await screen.findByRole("heading", { name: "Connect a language model" })).toBeDefined();
  expect(set).toEqual([]);
});

test("a picture chosen on first run is stored with the usual command", async () => {
  const user = userEvent.setup();
  const set: unknown[] = [];
  const { container } = renderApp("first=1&form=library", (api) => ({
    ...api,
    commands: {
      ...api.commands,
      setUserAvatar: (image) => {
        set.push(image);
        return api.commands.setUserAvatar(image);
      },
    },
  }));
  await user.type(await screen.findByLabelText("What should they call you?"), "Rowan");
  await user.click(screen.getByRole("button", { name: "Continue" }));
  await screen.findByRole("heading", { name: "Your picture" });
  const input = container.querySelector('input[type="file"]') as HTMLInputElement;
  await user.upload(input, new File(["x"], "me.png", { type: "image/png" }));
  await waitFor(() => expect(set.length).toBe(1));
  expect(await screen.findByRole("button", { name: "Continue" })).toBeDefined();
});

test("first run: every question is optional, and skipping them does not bring them back", async () => {
  const user = userEvent.setup();
  const { store } = renderApp("first=1&form=library");
  await screen.findByLabelText("What should they call you?");
  // Nothing filled in: Continue still works.
  const go = screen.getByRole("button", { name: "Continue" }) as HTMLButtonElement;
  expect(go.disabled).toBe(false);
  await user.click(go);
  await waitFor(() => expect(store.getState().status?.needs_setup).toBe(false));
  expect(store.getState().settings?.user_name).toBeNull();
});

const row = (name: string) => screen.getByText(name, { selector: "strong" }).closest("li") as HTMLElement;

test("the picker manages saves like the settings page: copy, rename, delete with a confirmation, export, import", async () => {
  const user = userEvent.setup();
  const onSwitched = vi.fn();
  renderWith(<SavePicker onSwitched={onSwitched} />, { query: "nosave=1" });
  await screen.findByText("second try", { selector: "strong" });
  // Nothing is in use, so nothing is marked so and every row can be deleted.
  expect(screen.queryByText("In use")).toBeNull();
  expect(screen.queryByText("Log out of this save")).toBeNull();

  await user.click(within(row("second try")).getByRole("button", { name: "Copy" }));
  await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "OK" }));
  await screen.findByText("second try 2", { selector: "strong" });

  await user.click(within(row("second try 2")).getByRole("button", { name: "Rename" }));
  const rename = await screen.findByRole("dialog");
  const field = within(rename).getByLabelText("New name");
  await user.clear(field);
  await user.type(field, "kept");
  await user.click(within(rename).getByRole("button", { name: "OK" }));
  await screen.findByText("kept", { selector: "strong" });

  const ask = vi.spyOn(window, "confirm").mockReturnValueOnce(false).mockReturnValueOnce(true);
  await user.click(within(row("kept")).getByRole("button", { name: "Delete" }));
  expect(screen.getByText("kept", { selector: "strong" })).toBeDefined();
  await user.click(within(row("kept")).getByRole("button", { name: "Delete" }));
  await waitFor(() => expect(screen.queryByText("kept", { selector: "strong" })).toBeNull());
  ask.mockRestore();

  await user.click(within(row("default")).getByRole("button", { name: "Export" }));
  const dialog = await screen.findByRole("dialog");
  await user.type(within(dialog).getByLabelText("Write to this file"), "/tmp/default.sqlite");
  await user.click(within(dialog).getByRole("button", { name: "OK" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

  // An imported file is opened, as a created save is.
  await user.type(screen.getByPlaceholderText("Path of the file"), "/tmp/old story.sqlite");
  await user.click(screen.getByRole("button", { name: "Import" }));
  await waitFor(() => expect(onSwitched).toHaveBeenCalledOnce());
});

test("new save: back on the first step returns to the picker and leaves no empty save", async () => {
  sessionStorage.clear();
  const user = userEvent.setup();
  const { api, store } = renderApp("nosave=1&first=1");
  await user.type(await screen.findByLabelText("Name of the new save"), "fresh");
  await user.click(screen.getByRole("button", { name: "Create and open" }));
  await waitFor(() => expect(readFresh()).toEqual({ name: "fresh", from: null }));
  // The window reloads onto the new save; here the status is read again instead.
  await createActions(api, store).boot();
  await screen.findByLabelText("What should they call you?");
  await user.click(screen.getByRole("button", { name: "Back" }));
  expect(await screen.findByRole("heading", { name: "Choose a save" })).toBeDefined();
  expect(screen.queryByText("fresh", { selector: "strong" })).toBeNull();
  const list = await api.commands.saves();
  expect(list.status === "ok" && list.data.some((x) => x.name === "fresh")).toBe(false);
  expect(readFresh()).toBeNull();
});

test("without a new-save flow the first step has no back", async () => {
  sessionStorage.clear();
  renderApp("first=1&form=library");
  await screen.findByLabelText("What should they call you?");
  expect(screen.queryByRole("button", { name: "Back" })).toBeNull();
});

test("every first-run step after the first goes back one step, keeping the answers", async () => {
  sessionStorage.clear();
  const user = userEvent.setup();
  renderApp("first=1&form=library");
  await user.type(await screen.findByLabelText("What should they call you?"), "Rowan");
  await user.click(screen.getByRole("button", { name: "Continue" }));
  await screen.findByRole("heading", { name: "Your picture" });
  await user.click(screen.getByRole("button", { name: "Back" }));
  const name = (await screen.findByLabelText("What should they call you?")) as HTMLInputElement;
  expect(name.value).toBe("Rowan");
  await user.click(screen.getByRole("button", { name: "Continue" }));
  await user.click(await screen.findByRole("button", { name: "Keep the default" }));
  await screen.findByRole("heading", { name: "Connect a language model" });
  await user.click(screen.getByRole("button", { name: "Back" }));
  await screen.findByRole("heading", { name: "Your picture" });
  await user.click(screen.getByRole("button", { name: "Keep the default" }));
  await user.click(await screen.findByRole("button", { name: "Skip for now" }));
  await screen.findByRole("heading", { name: "Check for updates?" });
  await user.click(screen.getByRole("button", { name: "Back" }));
  await screen.findByRole("heading", { name: "Connect a language model" });
});

test("cancelling a save made from another one goes back to that one and deletes the empty save", async () => {
  sessionStorage.clear();
  const { api, store } = renderApp("first=1");
  const c = api.commands;
  await c.newSave("x");
  await c.switchSave("x");
  markFresh({ name: "x", from: "second try" });
  expect(await createActions(api, store).cancelFresh({ name: "x", from: "second try" })).toBe("switched");
  const status = await c.appStatus();
  expect(status.status === "ok" && status.data.save).toBe("second try");
  const list = await c.saves();
  expect(list.status === "ok" && list.data.some((x) => x.name === "x")).toBe(false);
  expect(readFresh()).toBeNull();
  clearFresh();
});
