import { act, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { renderApp } from "../test/render";

async function list() {
  const nav = await screen.findByRole("navigation", { name: "Chats" });
  // Wait for the previews: every channel's newest message is loaded for them.
  await within(nav).findByText(/Take the stairs by the fish market/);
  return nav;
}

test("the chat list holds the host, direct chats and groups, with the selected one marked", async () => {
  renderApp();
  const nav = await list();
  const rows = within(nav).getAllByRole("button");
  expect(rows.length).toBeGreaterThanOrEqual(3);
  expect(within(nav).getByRole("button", { name: /Ward rota/ })).toBeDefined();
  const selected = rows.filter((r) => r.getAttribute("aria-current") === "true");
  expect(selected).toHaveLength(1);
  expect(selected[0].textContent).toMatch(/Host/);
});

test("rows are ordered by last activity, and a new message moves its chat to the top with a badge", async () => {
  const { store } = renderApp();
  const nav = await list();
  const names = () => within(nav).getAllByRole("button").map((r) => r.querySelector(".chat-name")!.textContent);
  const before = names();
  const last = before[before.length - 1]!;
  const target = store.getState().channels.find((c) => (c.kind === "group" ? c.topic : c.participants[0]?.name) === last)!;
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
  const top = within(nav).getAllByRole("button")[0];
  expect(within(top).getByRole("img", { name: "1 unread" })).toBeDefined();
  // Opening it clears the count.
  expect(store.getState().unread[target.id]).toBe(1);
  await userEvent.setup().click(top);
  expect(store.getState().unread[target.id]).toBeUndefined();
});

test("people waiting on the user show as unread in their chat", async () => {
  renderApp();
  const nav = await list();
  const mira = within(nav).getAllByRole("button").find((r) => r.textContent?.includes("Mira Vale"))!;
  expect(within(mira).getByRole("img", { name: "1 unread" })).toBeDefined();
});
