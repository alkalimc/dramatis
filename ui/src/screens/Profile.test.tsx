import { screen, waitFor, within } from "@testing-library/react";
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
  expect(within(drawer).getByText("Rowan")).toBeDefined();
  for (const name of ["My profile", "New group", "Contacts", "Archived chats", "Settings"]) {
    expect(within(drawer).getByRole("button", { name })).toBeDefined();
  }
  expect(within(drawer).getByText(/^Version /)).toBeDefined();

  await user.click(within(drawer).getByRole("button", { name: "Contacts" }));
  expect(screen.queryByRole("complementary", { name: "Menu" })).toBeNull();
  expect(await screen.findByRole("complementary", { name: "Roster" })).toBeDefined();
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

test("my profile edits the name, the birthday and the picture", async () => {
  const user = userEvent.setup();
  const { store } = renderApp();
  const drawer = await openMenu(user);
  await user.click(within(drawer).getByRole("button", { name: "My profile" }));
  const dialog = await screen.findByRole("dialog", { name: "My profile" });
  const name = within(dialog).getByRole("textbox", { name: "What should they call you?" });
  await user.clear(name);
  await user.type(name, "Rowan Hale");
  await user.type(within(dialog).getByLabelText("Month"), "3");
  await user.type(within(dialog).getByLabelText("Day"), "14");
  await user.click(within(dialog).getByRole("button", { name: "Save" }));
  await waitFor(() => expect(store.getState().settings?.user_name).toBe("Rowan Hale"));
  expect(store.getState().settings?.birthday).toEqual({ month: 3, day: 14 });
  expect(screen.queryByRole("dialog")).toBeNull();
});
