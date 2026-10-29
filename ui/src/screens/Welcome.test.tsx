import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
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

test("first run ends with the model step, then the host speaks first", async () => {
  const user = userEvent.setup();
  renderApp("first=1&form=library");
  await user.type(await screen.findByLabelText("What should they call you?"), "Rowan");
  await user.click(screen.getByRole("button", { name: "Continue" }));
  await user.click(await screen.findByRole("button", { name: "Keep the default" }));
  expect(await screen.findByRole("heading", { name: "Connect a language model" })).toBeDefined();

  await user.type(screen.getByLabelText("Address (base URL)"), "api.example.com");
  await user.type(screen.getByLabelText("Model for the people"), "model-a");
  await user.click(screen.getByRole("button", { name: "Save and continue" }));
  expect(await screen.findByText("The address must start with http:// or https:// and contain no spaces.")).toBeDefined();

  const url = screen.getByLabelText("Address (base URL)");
  await user.clear(url);
  await user.type(url, "https://api.example.com/v1");
  await user.type(screen.getByLabelText("API key"), "sk-test");
  await user.click(screen.getByRole("button", { name: "Save and continue" }));
  expect(await within(await screen.findByRole("log")).findByText(/Hello, Rowan\./)).toBeDefined();
});

test("the model step can be skipped, and picking a starting point fills the form", async () => {
  const user = userEvent.setup();
  renderApp("first=1&form=library");
  await user.type(await screen.findByLabelText("What should they call you?"), "Rowan");
  await user.click(screen.getByRole("button", { name: "Continue" }));
  await user.click(await screen.findByRole("button", { name: "Keep the default" }));
  await user.selectOptions(await screen.findByLabelText("Start from"), "local");
  expect((screen.getByLabelText("Address (base URL)") as HTMLInputElement).value).toBe("http://127.0.0.1:8080/v1");
  await user.click(screen.getByRole("button", { name: "Skip for now" }));
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
