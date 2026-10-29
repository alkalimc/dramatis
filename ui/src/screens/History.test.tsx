import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { renderApp } from "../test/render";

test("a long channel opens on its newest page and pages older lines in until the start", async () => {
  const user = userEvent.setup();
  const slow = { timeout: 5000 };
  renderApp("long=1");
  const log = await screen.findByRole("log", {}, slow);
  // 120 older lines plus the seeded ones: the first page holds the newest 50 only.
  expect(await within(log).findByText("Older line 120", {}, slow)).toBeDefined();
  expect(within(log).queryByText("Older line 60")).toBeNull();
  const earlier = () => within(log).findByRole("button", { name: "Show earlier messages" }, slow);
  await user.click(await earlier());
  expect(await within(log).findByText("Older line 60", {}, slow)).toBeDefined();
  expect(within(log).queryByText("Older line 1")).toBeNull();
  // The next page is short, which reaches the start; then the button goes.
  await user.click(await earlier());
  expect(await within(log).findByText("Older line 1", {}, slow)).toBeDefined();
  expect(within(log).queryByRole("button", { name: "Show earlier messages" })).toBeNull();
});
