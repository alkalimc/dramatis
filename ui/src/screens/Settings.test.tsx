import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { renderApp } from "../test/render";

async function openSettings(user: ReturnType<typeof userEvent.setup>) {
  await screen.findByRole("heading", { name: "Who's waiting" });
  await user.click(screen.getByRole("button", { name: "Menu" }));
  await user.click(within(await screen.findByRole("complementary", { name: "Menu" })).getByRole("button", { name: "Settings" }));
  return screen.findByRole("dialog", { name: "Settings" });
}

test("settings is a card with a list of sections; each opens its page and back returns", async () => {
  const user = userEvent.setup();
  renderApp();
  const card = await openSettings(user);
  // The drawer is gone and the right panel stays closed: this is a card over the app.
  expect(screen.queryByRole("complementary")).toBeNull();
  await user.click(within(card).getByRole("button", { name: "Quiet hours and notifications" }));
  const page = within(card).getByRole("region", { name: "Quiet hours and notifications" });
  // Every control carries a visible label.
  expect(within(page).getByRole("switch", { name: "Do not disturb" })).toBeDefined();
  expect(within(page).getByLabelText("From")).toBeDefined();
  expect(within(page).getByLabelText("To")).toBeDefined();
  // The happenings moved to the quota page, where the intensity sets how many there can be.
  expect(within(page).queryByLabelText("Happenings on board")).toBeNull();
  await user.click(within(card).getByRole("button", { name: "Back" }));
  await user.click(within(card).getByRole("button", { name: /^Quota and intensity/ }));
  const quota = within(card).getByRole("region", { name: "Quota and intensity" });
  // Happenings on board: one a day until the user chooses another number.
  const events = (await within(quota).findByLabelText("Happenings on board")) as HTMLSelectElement;
  expect(events.value).toBe("1");
  await user.selectOptions(events, "0");
  expect((within(quota).getByLabelText("Happenings on board") as HTMLSelectElement).value).toBe("0");
  await user.click(within(card).getByRole("button", { name: "Back" }));
  expect(within(card).getByRole("button", { name: /^Saves/ })).toBeDefined();
  await user.click(within(card).getByRole("button", { name: "Close" }));
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("the search icon turns the list column into a search of this chat", async () => {
  const user = userEvent.setup();
  renderApp();
  await screen.findByRole("heading", { name: "Who's waiting" });
  await user.click(screen.getByRole("button", { name: "Search in this chat" }));
  const panel = await screen.findByRole("navigation", { name: "Search in this chat" });
  expect(within(panel).getByText("Search messages in")).toBeDefined();
  await user.type(within(panel).getByRole("searchbox"), "flood");
  expect((await within(panel).findAllByText(/flood/)).length).toBeGreaterThan(0);
  await user.click(within(panel).getByRole("button", { name: "Close" }));
  expect(screen.queryByRole("navigation", { name: "Search in this chat" })).toBeNull();
});

test("opening the card focuses the card itself, not an icon button", async () => {
  const user = userEvent.setup();
  renderApp();
  const card = await openSettings(user);
  expect(document.activeElement).toBe(card);
});
