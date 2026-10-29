import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ChannelSummary } from "../api";
import { channelTitle } from "../lib/chats";
import { renderApp } from "../test/render";

const rowsOf = (nav: HTMLElement) => [...nav.querySelectorAll<HTMLButtonElement>("button.chat-row")];

async function list() {
  const nav = await screen.findByRole("navigation", { name: "Chats" });
  // Wait for the previews: every channel's newest message is loaded for them.
  await within(nav).findByText(/Take the stairs by the fish market/);
  return nav;
}

test("the chat list holds the host, direct chats and groups, with the selected one marked", async () => {
  renderApp();
  const nav = await list();
  const rows = rowsOf(nav);
  expect(rows.length).toBeGreaterThanOrEqual(3);
  // A group is named after its members until it is renamed.
  expect(within(nav).getByRole("button", { name: /Oren Lark, Mira Vale, Tess Arden/ })).toBeDefined();
  const selected = rows.filter((r) => r.getAttribute("aria-current") === "true");
  expect(selected).toHaveLength(1);
  expect(selected[0].textContent).toMatch(/Host/);
});

test("rows are ordered by last activity, and a new message moves its chat to the top with a badge", async () => {
  const { store } = renderApp();
  const nav = await list();
  const names = () => rowsOf(nav).map((r) => r.querySelector(".chat-name")!.textContent);
  const before = names();
  const last = before[before.length - 1]!;
  const target = store.getState().channels.find((c) => channelTitle(c, store.getState().t) === last)!;
  await act(async () => {
    store.dispatch({
      type: "messageAdded",
      event: {
        message: {
          id: "fresh",
          channel: target.id,
          author: target.participants[0] ? { kind: "person", id: target.participants[0].id } : { kind: "host" },
          text: "Something new.",
          actions: [],
          attachment: null,
          picture_lost: false,
          cites: [],
          confidence: null,
          relayed: false,
          task: null,
          note: null,
          at: new Date(Date.now() + 60_000).toISOString(),
        },
      },
    });
  });
  expect(names()[0]).toBe(last);
  const top = rowsOf(nav)[0];
  expect(within(top.parentElement!).getByRole("button", { name: "1 unread" })).toBeDefined();
  // Opening it clears the count.
  expect(store.getState().unread[target.id]).toBe(1);
  await userEvent.setup().click(top);
  expect(store.getState().unread[target.id]).toBeUndefined();
});

test("people waiting on the user show as unread in their chat", async () => {
  renderApp();
  const nav = await list();
  const mira = rowsOf(nav).find((r) => r.querySelector(".chat-name")!.textContent === "Mira Vale")!;
  expect(within(mira.parentElement!).getByRole("button", { name: "1 unread" })).toBeDefined();
});

test("clicking the unread badge clears it without opening the chat", async () => {
  const { store } = renderApp();
  const nav = await list();
  const mira = rowsOf(nav).find((r) => r.querySelector(".chat-name")!.textContent === "Mira Vale")!;
  await userEvent.setup().click(within(mira.parentElement!).getByRole("button", { name: "1 unread" }));
  await waitFor(() => expect(within(mira.parentElement!).queryByRole("button", { name: "1 unread" })).toBeNull());
  expect(store.getState().channel).toBeNull();
});

test("search narrows the list by name", async () => {
  const user = userEvent.setup();
  renderApp();
  const nav = await list();
  await user.type(within(nav).getByRole("searchbox", { name: "Search" }), "tess");
  const names = rowsOf(nav).map((r) => r.querySelector(".chat-name")!.textContent);
  expect(names).toContain("Tess Arden");
  expect(names).not.toContain("Juno Reyes");
});

test("right click archives a chat; the archived entry opens them and unarchives", async () => {
  const user = userEvent.setup();
  renderApp();
  const nav = await list();
  const row = () => rowsOf(nav).find((r) => r.textContent?.includes("Juno Reyes"))!;
  await user.pointer({ keys: "[MouseRight]", target: row() });
  await user.click(await screen.findByRole("menuitem", { name: "Archive" }));
  await waitFor(() => expect(rowsOf(nav).some((r) => r.textContent?.includes("Juno Reyes"))).toBe(false));
  await user.click(within(nav).getByRole("button", { name: /Archived chats/ }));
  expect(within(nav).getByRole("heading", { name: "Archived chats" })).toBeDefined();
  await user.pointer({ keys: "[MouseRight]", target: row() });
  await user.click(await screen.findByRole("menuitem", { name: "Unarchive" }));
  await user.click(within(nav).getByRole("button", { name: "Back" }));
  await waitFor(() => expect(rowsOf(nav).some((r) => r.textContent?.includes("Juno Reyes"))).toBe(true));
});

test("a conversation opened before anyone spoke gets a row while it is open, and loses it after", async () => {
  const { store } = renderApp();
  const nav = await list();
  const row: ChannelSummary = { id: "dm:new", kind: "direct", origin: "system", participants: [{ id: "kael", name: "Kael Moor" }], topic: null, mode: "enabled", last_at: null, name: null, avatar: null, archived: false };
  await act(async () => {
    store.dispatch({ type: "temp", row });
    store.dispatch({ type: "open", channel: row.id });
  });
  expect(rowsOf(nav).some((r) => r.textContent?.includes("Kael Moor"))).toBe(true);
  await act(async () => {
    store.dispatch({ type: "open", channel: null });
  });
  expect(rowsOf(nav).some((r) => r.textContent?.includes("Kael Moor"))).toBe(false);
});

test("a second right click opens the menu of the new row instead of closing", async () => {
  const user = userEvent.setup();
  renderApp();
  const nav = await list();
  const find = (name: string) => rowsOf(nav).find((r) => r.textContent?.includes(name))!;
  await user.pointer({ keys: "[MouseRight]", target: find("Juno Reyes") });
  expect(await screen.findByRole("menu", { name: "Chat actions" })).toBeDefined();
  await user.pointer({ keys: "[MouseRight]", target: find("Tess Arden") });
  expect(screen.getAllByRole("menu", { name: "Chat actions" })).toHaveLength(1);
});

test("a group's menu renames it through the info panel", async () => {
  const user = userEvent.setup();
  renderApp();
  const nav = await list();
  const group = rowsOf(nav).find((r) => r.querySelector(".chat-name svg"))!;
  await user.pointer({ keys: "[MouseRight]", target: group });
  for (const name of ["Rename", "Change picture", "Delete group"]) expect(await screen.findByRole("menuitem", { name })).toBeDefined();
  await user.click(screen.getByRole("menuitem", { name: "Rename" }));
  expect(await screen.findByRole("textbox", { name: "Rename" })).toBeDefined();
});
