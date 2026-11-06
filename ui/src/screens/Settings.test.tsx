import { screen, waitFor, within } from "@testing-library/react";
import { vi } from "vitest";
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
  await user.click(within(card).getByRole("button", { name: "Chat settings" }));
  await user.click(within(card).getByRole("button", { name: "Quiet hours and notifications" }));
  const page = within(card).getByRole("region", { name: "Quiet hours and notifications" });
  // Every control carries a visible label.
  expect(within(page).getByRole("switch", { name: "Do not disturb" })).toBeDefined();
  expect(within(page).getByLabelText("From")).toBeDefined();
  expect(within(page).getByLabelText("To")).toBeDefined();
  // The happenings moved to the quota page, where the intensity sets how many there can be.
  expect(within(page).queryByLabelText("Happenings on board")).toBeNull();
  // Back goes up one level at a time: quiet hours, then chat settings, then the main list.
  await user.click(within(card).getByRole("button", { name: "Back" }));
  expect(within(card).getByRole("region", { name: "Chat settings" })).toBeDefined();
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

test("typing and sending: the choice is a pair of radios, the card row shows it, and the other key adds a line", async () => {
  const user = userEvent.setup();
  const { store } = renderApp();
  const card = await openSettings(user);
  await user.click(within(card).getByRole("button", { name: "Chat settings" }));
  await user.click(within(card).getByRole("button", { name: /^Typing and sending/ }));
  const page = within(card).getByRole("region", { name: "Typing and sending" });
  const mod = within(page).getByRole("radio", { name: /sends, Enter adds a line/ }) as HTMLInputElement;
  expect(mod.checked).toBe(true);
  await user.click(within(page).getByRole("radio", { name: /^Enter sends, .* adds a line/ }));
  expect(store.getState().sendKey).toBe("enter");
  await user.click(within(card).getByRole("button", { name: "Back" }));
  expect(within(within(card).getByRole("button", { name: /^Typing and sending/ })).getByText("Enter")).toBeDefined();
});

test("network: system by default; a manual proxy needs a good address, keeps its password out of sight, and can be switched back", async () => {
  const user = userEvent.setup();
  const { api } = renderApp();
  const set = vi.spyOn(api.commands, "setNetwork");
  const card = await openSettings(user);
  await user.click(within(card).getByRole("button", { name: "Advanced" }));
  await user.click(within(card).getByRole("button", { name: "Network" }));
  const page = within(card).getByRole("region", { name: "Network" });
  expect((await within(page).findByRole("radio", { name: /^System proxy/ }) as HTMLInputElement).checked).toBe(true);
  expect(within(page).queryByLabelText("Proxy address")).toBeNull();

  await user.click(within(page).getByRole("radio", { name: /^Manual proxy/ }));
  await user.type(within(page).getByLabelText("Proxy address"), "proxy.example:8080");
  await user.click(within(page).getByRole("button", { name: "Save" }));
  expect(await within(page).findByText(/The proxy address must be http/)).toBeDefined();

  await user.clear(within(page).getByLabelText("Proxy address"));
  await user.type(within(page).getByLabelText("Proxy address"), "socks5://10.0.0.2:1080");
  await user.type(within(page).getByLabelText("User name (optional)"), "me");
  await user.type(within(page).getByLabelText("Password (optional)"), "s3cret");
  await user.type(within(page).getByLabelText("Skip the proxy for"), ".corp");
  await user.click(within(page).getByRole("button", { name: "Save" }));
  expect(await within(page).findByText("Saved. The next request uses it.")).toBeDefined();
  expect(set).toHaveBeenLastCalledWith({ mode: "manual", proxy: "socks5://10.0.0.2:1080", username: "me", no_proxy: ".corp" }, "s3cret");
  // The password was handed over once and is not shown again.
  expect(screen.queryByDisplayValue("s3cret")).toBeNull();
  expect(within(page).getByText("A password is stored in the keychain.")).toBeDefined();

  // Saving again without touching the password leaves it alone (null); direct keeps the typed proxy for later.
  await user.click(within(page).getByRole("radio", { name: /^Direct connection/ }));
  await user.click(within(page).getByRole("button", { name: "Save" }));
  await waitFor(() => expect(set).toHaveBeenLastCalledWith(expect.objectContaining({ mode: "direct", proxy: "socks5://10.0.0.2:1080" }), null));
});

test("the plugins page lists plugins off by default and turns one on; an unusable one stays off", async () => {
  const user = userEvent.setup();
  renderApp();
  const card = await openSettings(user);
  await user.click(within(card).getByRole("button", { name: "Plugins" }));
  const page = within(card).getByRole("region", { name: "Plugins" });
  const dice = await within(page).findByRole("switch", { name: "dice" });
  expect((dice as HTMLInputElement).checked).toBe(false);
  expect(within(page).getByText(/Tools: dice__roll/)).toBeDefined();
  await user.click(dice);
  await waitFor(() => expect((within(page).getByRole("switch", { name: "dice" }) as HTMLInputElement).checked).toBe(true));
  const broken = within(page).getByRole("switch", { name: "broken" }) as HTMLInputElement;
  expect(broken.disabled).toBe(true);
  expect(within(page).getByText(/Cannot be used/)).toBeDefined();
});

test("every page is reachable from the main list, sub-pages through their parent, and each is one level down", async () => {
  const user = userEvent.setup();
  renderApp();
  const card = await openSettings(user);
  const top = ["Chat settings", "Models and endpoints", "Quota and intensity", "Saves", "Privacy and security", "Plugins", "Advanced", "Language", "About"];
  for (const name of top) expect(within(card).getByRole("button", { name: new RegExp(`^${name}`) })).toBeDefined();
  expect(within(card).getByRole("button", { name: "Account and profile" })).toBeDefined();
  const tree: [string, string[]][] = [
    ["Chat settings", ["Night mode", "Typing and sending", "Quiet hours and notifications"]],
    ["Advanced", ["Data and storage", "Network"]],
  ];
  for (const [parent, kids] of tree) {
    for (const kid of kids) {
      await user.click(within(card).getByRole("button", { name: new RegExp(`^${parent}`) }));
      await user.click(within(card).getByRole("button", { name: new RegExp(`^${kid}`) }));
      expect(within(card).getByRole("region", { name: kid })).toBeDefined();
      await user.click(within(card).getByRole("button", { name: "Back" }));
      expect(within(card).getByRole("region", { name: parent })).toBeDefined();
      await user.click(within(card).getByRole("button", { name: "Back" }));
      expect(within(card).queryByRole("region")).toBeNull();
    }
  }
  for (const name of ["Models and endpoints", "Quota and intensity", "Saves", "Privacy and security", "Plugins", "Language", "About"]) {
    await user.click(within(card).getByRole("button", { name: new RegExp(`^${name}`) }));
    expect(within(card).getByRole("region", { name })).toBeDefined();
    await user.click(within(card).getByRole("button", { name: "Back" }));
  }
  // The main list has no back arrow.
  expect(within(card).queryByRole("button", { name: "Back" })).toBeNull();
});

test("Backspace goes up a level except while typing; Escape closes the card from any level", async () => {
  const user = userEvent.setup();
  renderApp();
  const card = await openSettings(user);
  await user.click(within(card).getByRole("button", { name: "Advanced" }));
  await user.click(within(card).getByRole("button", { name: "Network" }));
  await user.click(await within(card).findByRole("radio", { name: /^Manual proxy/ }));
  const address = within(card).getByLabelText("Proxy address");
  await user.type(address, "ab{Backspace}");
  expect((address as HTMLInputElement).value).toBe("a");
  expect(within(card).getByRole("region", { name: "Network" })).toBeDefined();
  // Focus on a button: Backspace leaves the page.
  within(card).getByRole("button", { name: "Back" }).focus();
  await user.keyboard("{Backspace}");
  expect(within(card).getByRole("region", { name: "Advanced" })).toBeDefined();
  await user.keyboard("{Backspace}");
  expect(within(card).queryByRole("region")).toBeNull();
  await user.click(within(card).getByRole("button", { name: /^Language/ }));
  await user.keyboard("{Escape}");
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("night mode is a page of Chat settings: follow the system, light or dark", async () => {
  const user = userEvent.setup();
  const { store } = renderApp();
  const card = await openSettings(user);
  await user.click(within(card).getByRole("button", { name: "Chat settings" }));
  expect(within(within(card).getByRole("button", { name: /^Night mode/ })).getByText("Follow the system")).toBeDefined();
  await user.click(within(card).getByRole("button", { name: /^Night mode/ }));
  const page = within(card).getByRole("region", { name: "Night mode" });
  expect((within(page).getByRole("radio", { name: /^Follow the system/ }) as HTMLInputElement).checked).toBe(true);
  await user.click(within(page).getByRole("radio", { name: "Dark" }));
  expect(store.getState().theme).toBe("dark");
  await user.click(within(page).getByRole("radio", { name: "Light" }));
  expect(store.getState().theme).toBe("light");
});
