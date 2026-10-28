import { screen, waitFor, within } from "@testing-library/react";
import { vi } from "vitest";
import userEvent from "@testing-library/user-event";
import { renderApp } from "../test/render";

async function openMenu(user: ReturnType<typeof userEvent.setup>) {
  await screen.findByRole("heading", { name: "Who's waiting" });
  await user.click(screen.getByRole("button", { name: "Menu" }));
  return screen.findByRole("complementary", { name: "Menu" });
}

test("the hamburger opens the drawer: the user, the places to go, night mode, the version", async () => {
  const user = userEvent.setup();
  renderApp();
  const drawer = await openMenu(user);
  expect(within(drawer).getAllByText("Rowan").length).toBeGreaterThan(0);
  for (const name of ["My profile", "New group", "Contacts", "Archived chats", "Settings"]) {
    expect(within(drawer).getByRole("button", { name })).toBeDefined();
  }
  expect(within(drawer).getByText(/^Version /)).toBeDefined();

  await user.click(within(drawer).getByRole("button", { name: "Contacts" }));
  expect(screen.queryByRole("complementary", { name: "Menu" })).toBeNull();
  const contacts = await screen.findByRole("dialog", { name: "Contacts" });
  expect(await within(contacts).findByText("Mira Vale")).toBeDefined();
});

test("night mode switches the theme and the choice is kept", async () => {
  const user = userEvent.setup();
  const { store } = renderApp();
  const drawer = await openMenu(user);
  const night = within(drawer).getByRole("switch", { name: "Night mode" }) as HTMLInputElement;
  expect(night.checked).toBe(true);
  await user.click(night);
  expect(store.getState().theme).toBe("light");
  await waitFor(() => expect(document.documentElement.dataset.theme).toBe("light"));
  expect(JSON.parse(localStorage.getItem("ui.prefs")!).theme).toBe("light");
});

test("the rail carries the menu button on top and no settings tab", async () => {
  renderApp();
  await screen.findByRole("heading", { name: "Who's waiting" });
  const rail = screen.getByRole("navigation", { name: "Sections" });
  expect(within(rail).queryByRole("button", { name: "System" })).toBeNull();
  expect(rail.previousElementSibling?.getAttribute("aria-label")).toBe("Menu");
});

test("my profile shows the card, and the pencil edits the name, the birthday and the picture", async () => {
  const user = userEvent.setup();
  const { store } = renderApp();
  const drawer = await openMenu(user);
  await user.click(within(drawer).getByRole("button", { name: "My profile" }));
  const dialog = await screen.findByRole("dialog", { name: "My profile" });
  expect(within(dialog).getByText("Birthday")).toBeDefined();
  await user.click(within(dialog).getByRole("button", { name: "Edit" }));
  await user.click(within(dialog).getByRole("button", { name: /^Name/ }));
  const name = within(dialog).getByRole("textbox", { name: "What should they call you?" });
  await user.clear(name);
  await user.type(name, "Rowan Hale");
  await user.click(within(dialog).getByRole("button", { name: /^Birthday/ }));
  await user.type(within(dialog).getByLabelText("Month"), "3");
  await user.type(within(dialog).getByLabelText("Day"), "14");
  // The camera button hands the click to the hidden file input, which Tauri needs a real click for.
  const input = dialog.querySelector<HTMLInputElement>('input[type="file"]')!;
  const clicked = vi.fn();
  input.addEventListener("click", clicked);
  await user.click(within(dialog).getByRole("button", { name: "Change picture" }));
  await user.click(await screen.findByRole("menuitem", { name: "Change picture" }));
  expect(clicked).toHaveBeenCalled();
  await user.click(within(dialog).getByRole("button", { name: "Save" }));
  await waitFor(() => expect(store.getState().settings?.user_name).toBe("Rowan Hale"));
  expect(store.getState().settings?.birthday).toEqual({ month: 3, day: 14 });
  expect((await within(screen.getByRole("dialog")).findAllByText("Rowan Hale")).length).toBeGreaterThan(0);
});

test("the bio is edited with a counter, limited to 200 characters, and shown on the card", async () => {
  const user = userEvent.setup();
  const { store } = renderApp();
  const drawer = await openMenu(user);
  await user.click(within(drawer).getByRole("button", { name: "My profile" }));
  const dialog = await screen.findByRole("dialog", { name: "My profile" });
  await user.click(within(dialog).getByRole("button", { name: "Edit" }));
  const bio = within(dialog).getByRole("textbox", { name: "Bio" });
  expect(within(dialog).getByLabelText("200 characters left")).toBeDefined();
  await user.type(bio, "Keeps bees");
  expect(within(dialog).getByLabelText("190 characters left")).toBeDefined();
  // Over the limit: the counter goes negative and saving waits.
  await user.clear(bio);
  await user.click(bio);
  await user.paste("x".repeat(201));
  expect(within(dialog).getByLabelText("-1 characters left")).toBeDefined();
  expect((within(dialog).getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(true);
  await user.clear(bio);
  await user.type(bio, "Keeps bees on the roof.");
  await user.click(within(dialog).getByRole("button", { name: "Save" }));
  await waitFor(() => expect(store.getState().settings?.bio).toBe("Keeps bees on the roof."));
  expect(await within(dialog).findByText("Keeps bees on the roof.")).toBeDefined();
});
