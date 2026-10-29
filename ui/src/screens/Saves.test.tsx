import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { renderWith } from "../test/render";
import { SavesPage } from "./Saves";

async function open(onSwitched = vi.fn()) {
  const user = userEvent.setup();
  const view = renderWith(<SavesPage onSwitched={onSwitched} />);
  await screen.findByText("second try");
  return { user, onSwitched, ...view };
}

const row = (name: string) => screen.getByText(name).closest("li") as HTMLElement;

test("lists saves with the one in use marked, and no delete or open on it", async () => {
  await open();
  expect(within(row("default")).getByText("In use")).toBeDefined();
  expect(within(row("default")).queryByRole("button", { name: "Delete" })).toBeNull();
  expect(within(row("default")).queryByRole("button", { name: "Open" })).toBeNull();
  expect(within(row("second try")).getByRole("button", { name: "Open" })).toBeDefined();
});

test("opening another save reloads what is on screen", async () => {
  const { user, onSwitched } = await open();
  await user.click(within(row("second try")).getByRole("button", { name: "Open" }));
  await waitFor(() => expect(onSwitched).toHaveBeenCalledOnce());
});

test("new, copy and rename go through the engine's naming rules", async () => {
  const { user, store } = await open();
  await user.type(screen.getByPlaceholderText("Name"), "fresh");
  await user.click(screen.getByRole("button", { name: "Create" }));
  await screen.findByText("fresh");

  await user.type(screen.getByPlaceholderText("Name"), "FRESH");
  await user.click(screen.getByRole("button", { name: "Create" }));
  await waitFor(() => expect(store.getState().error).toEqual({ code: "exists" }));

  await user.click(within(row("fresh")).getByRole("button", { name: "Copy" }));
  const dialog = await screen.findByRole("dialog");
  expect((within(dialog).getByLabelText("Name of the copy") as HTMLInputElement).value).toBe("fresh 2");
  await user.click(within(dialog).getByRole("button", { name: "OK" }));
  await screen.findByText("fresh 2");

  await user.click(within(row("fresh 2")).getByRole("button", { name: "Rename" }));
  const rename = await screen.findByRole("dialog");
  const field = within(rename).getByLabelText("New name");
  await user.clear(field);
  await user.type(field, "renamed");
  await user.click(within(rename).getByRole("button", { name: "OK" }));
  await screen.findByText("renamed");
  expect(screen.queryByText("fresh 2")).toBeNull();
});

test("delete asks first", async () => {
  const { user } = await open();
  const ask = vi.spyOn(window, "confirm").mockReturnValueOnce(false).mockReturnValueOnce(true);
  await user.click(within(row("second try")).getByRole("button", { name: "Delete" }));
  expect(ask).toHaveBeenCalledOnce();
  expect(screen.getByText("second try")).toBeDefined();
  await user.click(within(row("second try")).getByRole("button", { name: "Delete" }));
  await waitFor(() => expect(screen.queryByText("second try")).toBeNull());
  ask.mockRestore();
});

test("export names the file; import adds a save", async () => {
  const { user } = await open();
  await user.click(within(row("default")).getByRole("button", { name: "Export" }));
  const dialog = await screen.findByRole("dialog");
  expect(within(dialog).getByText(/A file already there is replaced/)).toBeDefined();
  await user.type(within(dialog).getByLabelText("Write to this file"), "/tmp/default.sqlite");
  await user.click(within(dialog).getByRole("button", { name: "OK" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

  await user.type(screen.getByPlaceholderText("Path of the file"), "/tmp/old story.sqlite");
  await user.click(screen.getByRole("button", { name: "Import" }));
  expect(await screen.findByText("Added old story.")).toBeDefined();
  await screen.findAllByText("old story");
});

test("an encrypted save is marked, and opening one asks for its password once", async () => {
  const { user } = await open();
  // A password-protected save imported with its password is known to this window.
  await user.type(screen.getByPlaceholderText("Path of the file"), "/tmp/vault.sqlite");
  await user.click(screen.getByRole("button", { name: "Import" }));
  const dialog = await screen.findByRole("dialog");
  await user.type(within(dialog).getByLabelText("Password"), "nope");
  await user.click(within(dialog).getByRole("button", { name: "Unlock" }));
  expect(await within(dialog).findByText("Wrong password.")).toBeDefined();
  await user.type(within(dialog).getByLabelText("Password"), "hunter2");
  await user.click(within(dialog).getByRole("button", { name: "Unlock" }));
  expect(await screen.findByText("Added vault.")).toBeDefined();
  expect(within(row("vault")).getByText("Encrypted")).toBeDefined();
  expect(within(row("default")).queryByText("Encrypted")).toBeNull();
});

test("shows where the saves are kept and copies the path", async () => {
  const user = userEvent.setup();
  const write = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue();
  const { actions } = await open();
  await actions.refreshStatus();
  expect(await screen.findByText("/Users/demo/.dramatis/saves")).toBeDefined();
  await user.click(screen.getByRole("button", { name: "Copy path" }));
  expect(write).toHaveBeenCalledWith("/Users/demo/.dramatis/saves");
});

test("logging out closes the save and the status says none is open", async () => {
  const user = userEvent.setup();
  const { store, actions } = await open();
  await actions.refreshStatus();
  await user.click(await screen.findByRole("button", { name: "Log out of this save" }));
  await waitFor(() => expect(store.getState().status?.save).toBeNull());
});
