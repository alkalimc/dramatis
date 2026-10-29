import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { renderApp } from "../test/render";

test("a wrong password says so and clears the field; the right one opens the save", async () => {
  const user = userEvent.setup();
  const slow = { timeout: 5000 };
  renderApp("locked=1");
  const field = await screen.findByLabelText("Password", {}, slow);
  const submit = screen.getByRole("button", { name: "Unlock" });
  expect((submit as HTMLButtonElement).disabled).toBe(true);
  await user.type(field, "nope");
  await user.click(submit);
  expect(await screen.findByText("Wrong password.", {}, slow)).toBeDefined();
  expect((screen.getByLabelText("Password") as HTMLInputElement).value).toBe("");
  expect(screen.queryByRole("heading", { name: "Who's waiting" })).toBeNull();
  await user.type(screen.getByLabelText("Password"), "hunter2");
  await user.click(screen.getByRole("button", { name: "Unlock" }));
  expect(await screen.findByRole("heading", { name: "Who's waiting" }, slow)).toBeDefined();
  expect(screen.queryByLabelText("Password")).toBeNull();
});
