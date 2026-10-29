import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { renderApp } from "../test/render";

test("a save changed in another window stops the app and offers to reopen it", async () => {
  const user = userEvent.setup();
  renderApp("changed=1");
  expect(await screen.findByRole("heading", { name: "This save was changed in another window" })).toBeDefined();
  expect(screen.queryByRole("heading", { name: "Who's waiting" })).toBeNull();
  await user.click(screen.getByRole("button", { name: "Reopen this save" }));
  expect(await screen.findByRole("heading", { name: "Who's waiting" })).toBeDefined();
});

test("or to choose another save", async () => {
  const user = userEvent.setup();
  renderApp("changed=1");
  await user.click(await screen.findByRole("button", { name: "Choose another save" }));
  expect(await screen.findByRole("heading", { name: "Choose a save" })).toBeDefined();
});
