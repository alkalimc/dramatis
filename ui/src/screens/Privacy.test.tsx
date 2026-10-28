import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { renderApp } from "../test/render";

async function openPrivacy(user: ReturnType<typeof userEvent.setup>) {
  await screen.findByRole("heading", { name: "Who's waiting" });
  await user.click(screen.getByRole("button", { name: "Menu" }));
  await user.click(within(await screen.findByRole("complementary", { name: "Menu" })).getByRole("button", { name: "Settings" }));
  const card = await screen.findByRole("dialog", { name: "Settings" });
  await user.click(within(card).getByRole("button", { name: "Privacy and security" }));
  return within(await within(card).findByRole("region", { name: "Privacy and security" }));
}

test("a locked save shows only the unlock screen until the password is right", async () => {
  const user = userEvent.setup();
  renderApp("locked=1");
  await screen.findByRole("heading", { name: "Enter your password" });
  expect(screen.queryByRole("heading", { name: "Who's waiting" })).toBeNull();
  const field = screen.getByLabelText("Password");
  await user.type(field, "nope");
  await user.click(screen.getByRole("button", { name: "Unlock" }));
  expect(await screen.findByText("Wrong password.")).toBeDefined();
  expect((screen.getByLabelText("Password") as HTMLInputElement).value).toBe("");
  await user.type(screen.getByLabelText("Password"), "hunter2");
  await user.click(screen.getByRole("button", { name: "Unlock" }));
  await screen.findByRole("heading", { name: "Who's waiting" });
});

test("a password is set after an explicit confirmation, changed, and turned off", async () => {
  const user = userEvent.setup();
  renderApp();
  const page = await openPrivacy(user);
  await user.click(page.getByRole("button", { name: /^Password/ }));
  expect(page.getByText(/cannot be recovered/)).toBeDefined();
  const submit = page.getByRole("button", { name: "Submit" }) as HTMLButtonElement;
  await user.type(page.getByLabelText("New password"), "one");
  await user.type(page.getByLabelText("Repeat the password"), "on");
  expect(page.getByText("The two passwords differ.")).toBeDefined();
  await user.type(page.getByLabelText("Repeat the password"), "e");
  // Not before the warning is acknowledged.
  expect(submit.disabled).toBe(true);
  await user.click(page.getByRole("checkbox"));
  expect(submit.disabled).toBe(false);
  await user.click(submit);
  expect(await page.findByText("Password set. This save is encrypted.")).toBeDefined();
  expect(page.getByRole("button", { name: /^Password/ }).textContent).toContain("On");

  // Change: the current one is asked for, and a wrong one is said so.
  await user.click(page.getByRole("button", { name: /^Password/ }));
  await user.click(page.getByRole("button", { name: "Change password" }));
  await user.type(page.getByLabelText("Current password"), "bad");
  await user.type(page.getByLabelText("New password"), "two");
  await user.type(page.getByLabelText("Repeat the password"), "two");
  await user.click(page.getByRole("button", { name: "Submit" }));
  expect(await page.findByText("Wrong password.")).toBeDefined();
  await user.type(page.getByLabelText("Current password"), "one");
  await user.click(page.getByRole("button", { name: "Submit" }));
  expect(await page.findByText("Password changed.")).toBeDefined();

  // Off.
  await user.click(page.getByRole("button", { name: /^Password/ }));
  await user.click(page.getByRole("button", { name: "Turn off password" }));
  await user.type(page.getByLabelText("Current password"), "two");
  await user.click(page.getByRole("button", { name: "Submit" }));
  expect(await page.findByText("Password removed. This save is not encrypted.")).toBeDefined();
  await waitFor(() => expect(page.getByRole("button", { name: /^Password/ }).textContent).toContain("Off"));
});
