import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { renderApp } from "./test/render";

test("full form: header clock, waiting panel, host channel with action lines", async () => {
  renderApp();
  expect(await screen.findByRole("heading", { name: "Who's waiting" })).toBeDefined();
  expect(screen.getByLabelText("In-world date and local time")).toBeDefined();
  const panel = screen.getByRole("heading", { name: "Who's waiting" }).closest("section")!;
  expect(await within(panel).findByText("Mira Vale")).toBeDefined();
  expect(within(panel).getByText("birthday today")).toBeDefined();
  expect(await screen.findByText(/Host ▸ finding who knows/)).toBeDefined();
  // No unread badges or counts on the panel.
  expect(within(panel).queryByText(/^\d+$/)).toBeNull();
});

test("a direct channel renders scene lines apart from speech, with sources and read-aloud", async () => {
  const user = userEvent.setup();
  renderApp();
  const panel = (await screen.findByRole("heading", { name: "Who's waiting" })).closest("section")!;
  await user.click(await within(panel).findByText("Mira Vale"));
  const scene = await screen.findByText("She runs a finger down the index.");
  expect(scene.className).toBe("scene");
  expect(scene.closest(".bubble")).toBeNull();
  expect(screen.getAllByText("Mira Vale").length).toBeGreaterThan(0);
  expect(screen.getByText(/Mira Vale ▸ asking Oren Lark to take a look/)).toBeDefined();
  // Her own material folds under the bubble; the other passage is a quote card.
  expect(await screen.findByRole("button", { name: "Sources (1)" })).toBeDefined();
  expect(screen.getByText("From the archive")).toBeDefined();
  expect(screen.getAllByRole("button", { name: "Read aloud" }).length).toBeGreaterThan(0);
});

test("no read-aloud button without a tts role", async () => {
  const user = userEvent.setup();
  renderApp("tts=0");
  const panel = (await screen.findByRole("heading", { name: "Who's waiting" })).closest("section")!;
  await user.click(await within(panel).findByText("Mira Vale"));
  await screen.findByText("She runs a finger down the index.");
  expect(screen.queryByRole("button", { name: "Read aloud" })).toBeNull();
});

test("selection menu: where from, and who knows this", async () => {
  const user = userEvent.setup();
  renderApp();
  const panel = (await screen.findByRole("heading", { name: "Who's waiting" })).closest("section")!;
  await user.click(await within(panel).findByText("Tess Arden"));
  await screen.findByText(/Take the stairs by the fish market/);
  const actions = screen.getAllByRole("button", { name: "Actions for this message" });
  await user.click(actions[actions.length - 1]);
  const menu = screen.getByRole("region", { name: "About the selected text" });
  await user.click(within(menu).getByRole("button", { name: "Where is this from?" }));
  expect(await within(menu).findByText("Dock Routes/Upper Town")).toBeDefined();
  await user.click(within(menu).getByRole("button", { name: "Who knows this?" }));
  expect(await within(menu).findByText("She has run the dock routes for years.")).toBeDefined();
  await user.click(within(menu).getByRole("button", { name: "Have someone check both" }));
  const dialog = await screen.findByRole("dialog", { name: "Ask someone" });
  expect((within(dialog).getByLabelText("What do you want to know?") as HTMLTextAreaElement).value).toMatch(/Check this one/);
});

test("ask card: candidates with reasons, turns hidden under more", async () => {
  const user = userEvent.setup();
  renderApp();
  await screen.findByRole("heading", { name: "Who's waiting" });
  await user.click(screen.getAllByRole("button", { name: /Ask someone/ })[0]);
  const dialog = await screen.findByRole("dialog", { name: "Ask someone" });
  await user.type(within(dialog).getByLabelText("What do you want to know?"), "the tide gates repair");
  expect(await within(dialog).findByText("Her own reports describe the gate repairs.")).toBeDefined();
  expect(within(dialog).getByLabelText("Turns").closest("details")!.open).toBe(false);
  // Disabled people are never offered.
  expect(within(dialog).queryByText("Silas Crane")).toBeNull();
});

test("roster: bands, no numbers on rows, disabled collapsed", async () => {
  const user = userEvent.setup();
  renderApp();
  await screen.findByRole("heading", { name: "Who's waiting" });
  await user.click(screen.getByRole("button", { name: "Roster" }));
  const drawer = await screen.findByRole("complementary");
  expect(await within(drawer).findByRole("heading", { name: "Close" })).toBeDefined();
  const disabled = within(drawer).getByText("Disabled (1)").closest("details")!;
  expect(disabled.open).toBe(false);
  for (const row of drawer.querySelectorAll(".roster-row")) {
    expect(row.textContent).not.toMatch(/\b\d{2,3}\b/);
  }
  await user.click(within(drawer).getByRole("button", { name: "Mira Vale" }));
  expect(await within(drawer).findByText("Trust 137")).toBeDefined();
  expect(within(drawer).getByRole("group", { name: "Read-aloud voice" })).toBeDefined();
});

test("world drawer: visibility on every row, hurt shows the quote and its effect", async () => {
  const user = userEvent.setup();
  renderApp();
  await screen.findByRole("heading", { name: "Who's waiting" });
  await user.click(screen.getByRole("button", { name: "World" }));
  const drawer = await screen.findByRole("complementary");
  const rows = await within(drawer).findAllByRole("listitem");
  for (const row of rows.filter((r) => r.classList.contains("fact"))) {
    expect(row.querySelector(".fact-meta")!.textContent).not.toBe("");
  }
  expect(within(drawer).getByText("Mira Vale's trust went down because of this")).toBeDefined();
  await user.click(within(drawer).getByRole("button", { name: "Deleted" }));
  expect(await within(drawer).findByRole("button", { name: "Restore" })).toBeDefined();
});

test("library form: not-connected notice, search, roster", async () => {
  const user = userEvent.setup();
  renderApp("form=library");
  expect(await screen.findByText(/Not connected\./)).toBeDefined();
  expect(screen.queryByRole("heading", { name: "Who's waiting" })).toBeNull();
  await user.type(screen.getByRole("searchbox", { name: "Search the archive" }), "bridge");
  await user.click(screen.getByRole("button", { name: "Search" }));
  expect(await screen.findByText("Dock Routes · Upper Town")).toBeDefined();
  await user.click(screen.getByRole("button", { name: "Set up now" }));
  expect(await screen.findByRole("heading", { name: "Endpoints" })).toBeDefined();
});

test("first open: name, then the host introduces itself with the coldstart card", async () => {
  const user = userEvent.setup();
  renderApp("first=1");
  await user.type(await screen.findByLabelText("What should they call you?"), "Rowan");
  await user.click(screen.getByRole("button", { name: "Continue" }));
  expect(await screen.findByRole("heading", { name: "You could start with them" })).toBeDefined();
  await waitFor(() => expect(screen.getByText(/Hello, Rowan\./)).toBeDefined());
  expect(screen.getByText("They are all frozen now: they answer when you reach out.")).toBeDefined();
});

test("endpoints: a failed model fetch shows the endpoint's own text and a manual field", async () => {
  const user = userEvent.setup();
  renderApp();
  await screen.findByRole("heading", { name: "Who's waiting" });
  await user.click(screen.getByRole("button", { name: "System" }));
  await user.click(await screen.findByRole("tab", { name: "Endpoints" }));
  const card = (await screen.findByRole("heading", { name: "deepseek" })).closest("li")!;
  await user.click(within(card).getByRole("button", { name: "Fetch models" }));
  expect(await within(card).findByText(/401 Unauthorized/)).toBeDefined();
  expect(within(card).getByLabelText("Model name")).toBeDefined();
  // Storing a key leaves only the notice; the key is never shown again.
  await user.type(within(card).getByLabelText("API key"), "sk-test");
  await user.click(within(card).getByRole("button", { name: "Store key" }));
  expect(await within(card).findByText("Stored in keychain")).toBeDefined();
  expect(within(card).queryByDisplayValue("sk-test")).toBeNull();
});
