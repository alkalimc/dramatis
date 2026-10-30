import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ChannelSummary } from "../api";
import { channelTitle } from "../lib/chats";
import { renderApp, renderWith } from "../test/render";
import { ChatList } from "./ChatList";

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
  expect(within(nav).getByRole("button", { name: /Rowan, Oren Lark \(4 in all\)/ })).toBeDefined();
  const selected = rows.filter((r) => r.getAttribute("aria-current") === "true");
  expect(selected).toHaveLength(1);
  expect(selected[0].textContent).toMatch(/Host/);
});

test("rows are ordered by last activity, and a new message moves its chat to the top with a badge", async () => {
  const { store } = renderApp();
  const nav = await list();
  // The favorites stay pinned above everything; the order below them is the activity order.
  const names = () => rowsOf(nav).map((r) => r.querySelector(".chat-name")!.textContent).filter((n) => n !== "Favorites");
  const before = names();
  const last = before[before.length - 1]!;
  const target = store.getState().channels.find((c) => channelTitle(c, store.getState().t, "Rowan") === last)!;
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
         
          at: new Date(Date.now() + 60_000).toISOString(),
        },
      },
    });
  });
  expect(names()[0]).toBe(last);
  const top = rowsOf(nav).find((r) => r.querySelector(".chat-name")!.textContent === last)!;
  const badge = within(top.parentElement!).getByRole("button", { name: "1 unread" });
  // The shared pill, beside the row rather than inside its time line.
  expect(badge.className).toBe("badge");
  expect(badge.parentElement).toBe(top.parentElement);
  expect(top.contains(badge)).toBe(false);
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
  const row: ChannelSummary = { id: "dm:new", kind: "direct", origin: "system", participants: [{ id: "kael", name: "Kael Moor" }], topic: null, mode: "enabled", last_at: null, name: null, avatar: null, archived: false, unread: 0 };
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
  const find = (name: string) => rowsOf(nav).find((r) => r.querySelector(".chat-title")?.textContent === name)!;
  await user.pointer({ keys: "[MouseRight]", target: find("Juno Reyes") });
  expect(await screen.findByRole("menu", { name: "Chat actions" })).toBeDefined();
  await user.pointer({ keys: "[MouseRight]", target: find("Tess Arden") });
  expect(screen.getAllByRole("menu", { name: "Chat actions" })).toHaveLength(1);
});

test("a group's menu holds the chat actions; renaming and the picture live in its info panel", async () => {
  const user = userEvent.setup();
  renderApp();
  const nav = await list();
  const group = rowsOf(nav).find((r) => r.querySelector(".chat-name svg"))!;
  await user.pointer({ keys: "[MouseRight]", target: group });
  expect(await screen.findByRole("menuitem", { name: "Delete group" })).toBeDefined();
  expect(screen.queryByRole("menuitem", { name: "Rename" })).toBeNull();
  expect(screen.queryByRole("menuitem", { name: "Change picture" })).toBeNull();
});

/** The list on the stand-in engine, which hides the conversations in `hidden` as the real one hides empty ones. */
function emptyAware(hidden: Set<string>) {
  return renderWith(<ChatList />, {
    patch: (api) => ({
      ...api,
      commands: {
        ...api.commands,
        channels: async () => {
          const r = await api.commands.channels();
          return r.status === "ok" ? { status: "ok", data: r.data.filter((c) => !hidden.has(c.id)) } : r;
        },
      },
    }),
  });
}

test("a group made from an offer appears at once, is opened, and follows the temporary-row rule", async () => {
  const hidden = new Set<string>();
  const { actions, api, store } = emptyAware(hidden);
  await act(async () => {
    await actions.refreshChannels();
  });
  const mira = { id: "mira", name: "Mira Vale" };
  const oren = { id: "oren", name: "Oren Lark" };
  const made = await api.commands.createGroup(["mira", "oren"], "plan a party");
  if (made.status !== "ok") throw new Error("no group");
  // Nobody has said anything in it yet, so the engine does not list it.
  hidden.add(made.data);
  await act(async () => {
    await actions.open(made.data, [mira, oren]);
  });
  const nav = screen.getByRole("navigation", { name: "Chats" });
  const row = () => rowsOf(nav).find((r) => r.textContent?.includes("Mira Vale, Oren Lark"));
  expect(store.getState().channel).toBe(made.data);
  expect(row()).toBeDefined();
  expect(row()!.getAttribute("aria-current")).toBe("true");
  // Left unspoken, the row goes when the user leaves.
  await act(async () => {
    await actions.open(null);
  });
  expect(row()).toBeUndefined();
  // Once something is said it is listed like any other.
  hidden.clear();
  await act(async () => {
    await actions.refreshChannels();
  });
  expect(row()).toBeDefined();
});

test("a person found for the user gets a row when opened before anyone spoke", async () => {
  const hidden = new Set<string>();
  const { actions, store } = emptyAware(hidden);
  await act(async () => {
    await actions.refreshChannels();
  });
  const juno = store.getState().channels.find((c) => c.kind === "direct" && c.participants.some((p) => p.name === "Juno Reyes"))!;
  hidden.add(juno.id);
  await act(async () => {
    await actions.refreshChannels();
    await actions.open(juno.id, { id: juno.participants[0].id, name: "Juno Reyes" });
  });
  const nav = screen.getByRole("navigation", { name: "Chats" });
  expect(rowsOf(nav).some((r) => r.textContent?.includes("Juno Reyes") && r.getAttribute("aria-current") === "true")).toBe(true);
});

test("opening an archived conversation from anywhere shows the archived list with it selected", async () => {
  const { actions, store, api } = renderWith(<ChatList />);
  await act(async () => {
    await actions.refreshChannels();
  });
  const juno = store.getState().channels.find((c) => c.kind === "direct" && c.participants.some((p) => p.name === "Juno Reyes"))!;
  await api.commands.archiveChannel(juno.id, true);
  await act(async () => {
    await actions.refreshChannels();
    await actions.open(juno.id, juno.participants[0]);
  });
  const nav = screen.getByRole("navigation", { name: "Chats" });
  expect(within(nav).getByRole("heading", { name: "Archived chats" })).toBeDefined();
  const selected = rowsOf(nav).filter((r) => r.getAttribute("aria-current") === "true");
  expect(selected).toHaveLength(1);
  expect(selected[0].textContent).toContain("Juno Reyes");
  // It stays archived.
  expect(store.getState().channels.find((c) => c.id === juno.id)?.archived).toBe(true);
  // Going back to the host returns the list to the main view.
  await act(async () => {
    await actions.open(null);
  });
  expect(within(nav).queryByRole("heading", { name: "Archived chats" })).toBeNull();
});

test("unsent text stays as a draft: the list shows it, and coming back finds it in the box", async () => {
  const user = userEvent.setup();
  const { store } = renderApp();
  const nav = await list();
  const row = (name: string) => rowsOf(nav).find((r) => r.querySelector(".chat-title")?.textContent === name)!;
  const box = () => screen.getByRole("textbox", { name: "Message" }) as HTMLTextAreaElement;
  await user.type(box(), "half a thought");
  await user.click(row("Mira Vale"));
  await waitFor(() => expect(store.getState().channel).toBe("dm:mira"));
  expect(await within(row("Host")).findByText("Draft: half a thought")).toBeDefined();
  await waitFor(() => expect(box().value).toBe(""));
  await user.click(row("Host"));
  await waitFor(() => expect(box().value).toBe("half a thought"));
  // The open conversation shows its last message, not its own draft; sending clears it.
  expect(within(row("Host")).queryByText(/^Draft:/)).toBeNull();
  await user.click(box());
  await user.keyboard("{Meta>}{Enter}{/Meta}");
  await waitFor(() => expect(box().value).toBe(""));
  await user.click(row("Mira Vale"));
  await waitFor(() => expect(store.getState().channel).toBe("dm:mira"));
  expect(within(nav).queryByText(/^Draft:/)).toBeNull();
});

test("the favorites are the first row with a bookmark, whatever the other chats do, and have no row menu", async () => {
  const user = userEvent.setup();
  renderApp();
  const nav = await list();
  const first = rowsOf(nav)[0];
  expect(first.querySelector(".chat-title")!.textContent).toBe("Favorites");
  expect(first.querySelector(".avatar-favorites .icon-svg")).not.toBeNull();
  // Pinned above even the pinned ones, and read: no badge, no menu.
  expect(within(first.parentElement!).queryByRole("button", { name: /unread/ })).toBeNull();
  await user.pointer({ keys: "[MouseRight]", target: first });
  expect(screen.queryByRole("menu")).toBeNull();
});

test("a kept message shows its first author and the time it was first said, and can be deleted from its menu", async () => {
  const user = userEvent.setup();
  const { store } = renderApp();
  const nav = await list();
  await user.click(rowsOf(nav)[0]);
  const log = await screen.findByRole("log");
  const kept = await within(log).findByText(/The market closes at dusk/);
  const article = kept.closest("article")!;
  // As if Tess had written it here: her name, no forward header.
  expect(article.getAttribute("aria-label")).toBe("Tess Arden");
  expect(within(article).queryByText(/Forwarded from/)).toBeNull();
  // The small time is the original one (three days back), not when it was kept.
  const fav = store.getState().channels.find((c) => c.kind === "favorites")!;
  const message = store.getState().messages[fav.id].find((m) => m.text.includes("market closes"))!;
  expect(article.querySelector("time")!.getAttribute("datetime")).toBe(message.original_at);
  expect(message.original_at).not.toBe(message.at);

  // The menu ends with Delete here, and it asks first.
  const confirm = vi.spyOn(window, "confirm").mockReturnValueOnce(false).mockReturnValueOnce(true);
  await user.pointer({ keys: "[MouseRight]", target: article });
  let menu = await screen.findByRole("menu", { name: "Actions for this message" });
  expect(within(menu).getAllByRole("menuitem").map((i) => i.textContent)).toEqual(["Reply", "Copy text", "Forward", "Delete"]);
  await user.click(within(menu).getByRole("menuitem", { name: "Delete" }));
  expect(confirm).toHaveBeenCalledTimes(1);
  expect(within(log).getByText(/The market closes at dusk/)).toBeDefined();
  await user.pointer({ keys: "[MouseRight]", target: article });
  menu = await screen.findByRole("menu", { name: "Actions for this message" });
  await user.click(within(menu).getByRole("menuitem", { name: "Delete" }));
  await waitFor(() => expect(within(log).queryByText(/The market closes at dusk/)).toBeNull());
  expect(within(log).getByText(/Nothing kept yet/)).toBeDefined();
});

test("a note typed in the favorites stays there without an answer, and there is no more menu or archive for them", async () => {
  const user = userEvent.setup();
  const { store } = renderApp();
  const nav = await list();
  await user.click(rowsOf(nav)[0]);
  await screen.findByRole("log");
  expect(screen.queryByRole("button", { name: "More" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Group info" })).toBeNull();
  const fav = store.getState().channels.find((c) => c.kind === "favorites")!;
  expect(screen.getByRole("heading", { name: "Favorites" })).toBeDefined();
  expect(screen.getByText("Messages you kept")).toBeDefined();
  expect(fav.participants).toEqual([]);
});
