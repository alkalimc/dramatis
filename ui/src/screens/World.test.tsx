import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { renderApp, renderWith } from "../test/render";
import { WorldPage } from "./World";

test("the event log lists what happened, with who was part of it", async () => {
  renderWith(<WorldPage />);
  const log = await screen.findByRole("region", { name: "On board lately" });
  expect(within(log).getByText("Tea on the deck")).toBeDefined();
  expect(within(log).getByText(/With Mira Vale, /)).toBeDefined();
});

test("a happening is one memory: it shows once, and deleting it leaves the list and comes back from Deleted", async () => {
  const user = userEvent.setup();
  renderWith(<WorldPage />);
  const log = await screen.findByRole("region", { name: "On board lately" });
  // Its fact is not listed a second time.
  expect(screen.queryAllByText(/Tea on the deck/).length).toBe(1);
  const card = within(log).getByText("Tea on the deck").closest("li")!;
  await user.click(within(card).getByRole("button", { name: "Delete" }));
  await waitFor(() => expect(screen.queryByText("Tea on the deck")).toBeNull());
  await user.click(screen.getByRole("button", { name: "Deleted" }));
  const gone = (await screen.findByText("Tea on the deck")).closest("li")!;
  await user.click(within(gone).getByRole("button", { name: "Restore" }));
  await waitFor(() => expect(screen.queryByText("Tea on the deck")).toBeNull());
  await user.click(screen.getByRole("button", { name: "All" }));
  expect(await screen.findByText("Tea on the deck")).toBeDefined();
});

test("deleting a happening's fact takes the happening out of recently on board too", async () => {
  const user = userEvent.setup();
  const { api } = renderApp();
  await screen.findByRole("heading", { name: "Who's waiting" });
  await user.click(screen.getByRole("button", { name: "World" }));
  await screen.findByText("Tea on the deck");
  const list = await api.commands.facts({ scope: "world", person: null });
  const fact = list.status === "ok" ? list.data.items.find((f) => f.text.startsWith("Tea on the deck")) : undefined;
  expect(fact).toBeDefined();
  await api.commands.deleteFact(fact!.id);
  await waitFor(() => expect(screen.queryByText("Tea on the deck")).toBeNull());
});

test("recently on board holds the newest memories of each category, each one deletable", async () => {
  const user = userEvent.setup();
  renderWith(<WorldPage />);
  const log = await screen.findByRole("region", { name: "On board lately" });
  const groups = within(log).getAllByRole("group").map((g) => g.getAttribute("aria-label"));
  expect(groups).toEqual(expect.arrayContaining(["Happenings", "Said", "Private", "Promises", "Relationship"]));
  const promises = within(log).getByRole("group", { name: "Promises" });
  const row = within(promises).getByText(/bring Oren the new rota/).closest("li")!;
  await user.click(within(row).getByRole("button", { name: "Delete" }));
  await waitFor(() => expect(screen.queryByText(/bring Oren the new rota/)).toBeNull());
  // Listed once while live: a memory in a card is not repeated in the list under it.
  expect(screen.queryAllByText(/Oren mentioned the east ward/).length).toBe(1);
});
