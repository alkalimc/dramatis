import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { Message } from "../api";
import { renderApp } from "../test/render";

const rowsOf = (nav: HTMLElement) => [...nav.querySelectorAll<HTMLButtonElement>("button.chat-row")];

async function list() {
  const nav = await screen.findByRole("navigation", { name: "Chats" });
  await within(nav).findByText(/Take the stairs by the fish market/);
  return nav;
}

/** Open the forward picker for a message of the host's channel, from its context menu. */
async function forwardPicker(user: ReturnType<typeof userEvent.setup>) {
  const log = await screen.findByRole("log");
  const line = await within(log).findByText(/Mira Vale keeps the flood ledgers/);
  await user.pointer({ keys: "[MouseRight]", target: line.closest("article")! });
  await user.click(await screen.findByRole("menuitem", { name: "Forward" }));
  return screen.findByRole("dialog", { name: "Forward to" });
}

test("the picker lists the favorites, the host and every person, the one in the current chat included", async () => {
  const user = userEvent.setup();
  renderApp();
  await list();
  const dialog = await forwardPicker(user);
  const names = within(dialog).getAllByRole("button").map((b) => b.querySelector("span:last-child")!.textContent);
  expect(names.slice(0, 2)).toEqual(["Favorites", "Host"]);
  expect(names).toContain("Mira Vale");
  expect(names).toContain("Oren Lark");
  await user.type(within(dialog).getByRole("searchbox"), "ti");
  expect(within(dialog).queryByRole("button", { name: "Favorites" })).toBeNull();
  await user.clear(within(dialog).getByRole("searchbox"));
  await user.type(within(dialog).getByRole("searchbox"), "zzz");
  expect(within(dialog).getByText("No one matches.")).toBeDefined();
});

test("forwarding to a person opens their chat with a copy that names the original author, and they answer", async () => {
  const user = userEvent.setup();
  const { store } = renderApp();
  await list();
  const dialog = await forwardPicker(user);
  await user.click(within(dialog).getByRole("button", { name: "Oren Lark" }));
  await waitFor(() => expect(store.getState().channel).toBe("dm:oren"));
  expect(screen.queryByRole("dialog", { name: "Forward to" })).toBeNull();
  const log = await screen.findByRole("log");
  const copy = await within(log).findByText(/Mira Vale keeps the flood ledgers/);
  // The host wrote it: the copy says so.
  expect(within(copy.closest("article")!).getByText("Forwarded from Host")).toBeDefined();
  // Oren answers it like a message.
  await waitFor(() => expect(store.getState().messages["dm:oren"].filter((m) => m.author.kind === "person").length).toBeGreaterThan(1), { timeout: 5000 });
});

test("forwarding to the favorites keeps the chat on screen and the message there under its own author", async () => {
  const user = userEvent.setup();
  const { store } = renderApp();
  const nav = await list();
  const dialog = await forwardPicker(user);
  await user.click(within(dialog).getByRole("button", { name: "Favorites" }));
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Forward to" })).toBeNull());
  expect(store.getState().channel).toBeNull();
  const fav = store.getState().channels.find((c) => c.kind === "favorites")!;
  await waitFor(() => expect(store.getState().messages[fav.id].some((m: Message) => m.text.includes("flood ledgers"))).toBe(true));
  // No header, the author as the author, and nothing unread.
  await user.click(rowsOf(nav).find((r) => r.querySelector(".chat-title")?.textContent === "Favorites")!);
  const log = await screen.findByRole("log");
  const kept = await within(log).findByText(/Mira Vale keeps the flood ledgers/);
  expect(within(kept.closest("article")!).queryByText(/Forwarded from/)).toBeNull();
  expect(kept.closest("article")!.getAttribute("aria-label")).toBe("Host");
  expect(store.getState().unread[fav.id]).toBeUndefined();
});

test("a refused forward leaves the picker open and says why", async () => {
  const user = userEvent.setup();
  const { store } = renderApp("band=exhausted");
  await list();
  const dialog = await forwardPicker(user);
  await user.click(within(dialog).getByRole("button", { name: "Oren Lark" }));
  await waitFor(() => expect(store.getState().error?.code).toBe("quota_exhausted"));
  expect(screen.getByRole("dialog", { name: "Forward to" })).toBeDefined();
  // The favorites need no model: they still work.
  await user.click(within(screen.getByRole("dialog", { name: "Forward to" })).getByRole("button", { name: "Favorites" }));
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Forward to" })).toBeNull());
});
