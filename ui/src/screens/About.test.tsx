import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { version } from "../../package.json";
import { renderApp } from "../test/render";

async function openAbout(user: ReturnType<typeof userEvent.setup>) {
  await screen.findByRole("heading", { name: "Who's waiting" });
  await user.click(screen.getByRole("button", { name: "Menu" }));
  const drawer = await screen.findByRole("complementary", { name: "Menu" });
  await waitFor(() => expect(within(drawer).getByText(`Version ${version}`)).toBeDefined());
  await user.click(within(drawer).getByRole("button", { name: new RegExp(`Dramatis\\s*Version ${version}`) }));
  return screen.findByRole("region", { name: "About" });
}

test("the drawer footer opens About: name, version, author, links, licence and the corpus credit", async () => {
  const user = userEvent.setup();
  renderApp();
  const page = await openAbout(user);
  expect(within(page).getByRole("heading", { name: "Dramatis" })).toBeDefined();
  expect(within(page).getByText(`Version ${version}`)).toBeDefined();
  // The person links to their profile; the project's links go to the repository.
  expect(within(page).getByRole("link", { name: "Made by alkali" }).getAttribute("href")).toBe("https://github.com/alkalimc");
  expect(within(page).getByRole("link", { name: "Source code on GitHub" }).getAttribute("href")).toBe("https://github.com/alkalimc/dramatis");
  expect(within(page).getByRole("link", { name: "Report a problem" }).getAttribute("href")).toBe("https://github.com/alkalimc/dramatis/issues");
  expect(within(page).getByText("Licence: Apache-2.0")).toBeDefined();
  const corpus = within(page).getByRole("link", { name: "Corpus built by the demo team · CC BY-NC-SA 4.0" });
  expect(corpus.getAttribute("href")).toBe("https://example.org/demo-team");
  expect(within(page).getByRole("link", { name: "Corpus project" }).getAttribute("href")).toBe("https://example.org/demo-corpus");
});

test("Check now shows the newer release with its page, and the drawer says so", async () => {
  const user = userEvent.setup();
  renderApp();
  const page = await openAbout(user);
  expect(within(page).getByText("Not checked yet.")).toBeDefined();
  await user.click(within(page).getByRole("button", { name: "Check now" }));
  expect(await within(page).findByText(/Version 99\.0\.0 is available\./)).toBeDefined();
  expect(within(page).getByRole("link", { name: "View the release" }).getAttribute("href")).toBe("https://github.com/alkalimc/dramatis/releases/tag/v99.0.0");
  // The drawer carries a quiet note: no dialog came up.
  expect(screen.queryByRole("alertdialog")).toBeNull();
  await user.click(within(screen.getByRole("dialog", { name: "Settings" })).getByRole("button", { name: "Close" }));
  await user.click(screen.getByRole("button", { name: "Menu" }));
  expect(within(await screen.findByRole("complementary", { name: "Menu" })).getByText("Update available")).toBeDefined();
});

test("a check that cannot be made is reported on this page only", async () => {
  const user = userEvent.setup();
  const { store } = renderApp("update=fail");
  const page = await openAbout(user);
  await user.click(within(page).getByRole("button", { name: "Check now" }));
  expect(await within(page).findByText("Could not check for updates.")).toBeDefined();
  expect(store.getState().error).toBeNull();
});

test("automatic checks are off until the person turns them on", async () => {
  const user = userEvent.setup();
  const { store } = renderApp();
  const page = await openAbout(user);
  const auto = within(page).getByRole("switch", { name: "Check for updates once a day" }) as HTMLInputElement;
  expect(auto.checked).toBe(false);
  await user.click(auto);
  await waitFor(() => expect(store.getState().update?.auto).toBe(true));
  expect((within(page).getByRole("switch", { name: "Check for updates once a day" }) as HTMLInputElement).checked).toBe(true);
});

test("a release found earlier shows in the drawer at once", async () => {
  const user = userEvent.setup();
  renderApp("update=1");
  await screen.findByRole("heading", { name: "Who's waiting" });
  await user.click(screen.getByRole("button", { name: "Menu" }));
  expect(await screen.findByText("Update available")).toBeDefined();
});

test("a home that was never asked is asked once, and the answer is kept", async () => {
  const user = userEvent.setup();
  const { store } = renderApp("update=ask");
  expect(await screen.findByRole("heading", { name: "Check for updates?" })).toBeDefined();
  expect(screen.getByText(/Nothing is downloaded/)).toBeDefined();
  await user.click(screen.getByRole("button", { name: "Turn on" }));
  expect(await screen.findByRole("heading", { name: "Who's waiting" })).toBeDefined();
  expect(store.getState().update).toMatchObject({ auto: true, asked: true });
});
