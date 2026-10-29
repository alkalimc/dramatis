import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { renderApp } from "./test/render";

test("full form: header clock, waiting panel, host channel with action lines", async () => {
  renderApp();
  expect(await screen.findByRole("heading", { name: "Who's waiting" })).toBeDefined();
  // The in-world date and time is one quiet line under the chat list.
  expect(screen.getByLabelText("In-world date and local time").closest("footer")).not.toBeNull();
  const panel = screen.getByRole("heading", { name: "Who's waiting" }).closest("section")!;
  expect(await within(panel).findByText("Mira Vale")).toBeDefined();
  // One pinned line at first; the list opens with two rows and the rest are one click away.
  expect(within(panel).queryByText("birthday today")).toBeNull();
  const user = userEvent.setup();
  await user.click(within(panel).getByRole("button", { name: "Show the list" }));
  expect(within(panel).queryByText("birthday today")).toBeNull();
  await user.click(within(panel).getByRole("button", { name: "Show 2 more" }));
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
  // The scene line sits inside a bubble, apart from the spoken text.
  expect(scene.className).toBe("scene");
  expect(scene.closest(".bubble")).not.toBeNull();
  expect(scene.closest(".speech")).toBeNull();
  expect(screen.getAllByText("Mira Vale").length).toBeGreaterThan(0);
  expect(screen.getByText(/Mira Vale ▸ asking Oren Lark to take a look/)).toBeDefined();
  // Sources start folded; the button opens a panel with the cited original text and a link.
  const toggle = await screen.findByRole("button", { name: "Sources (2)" });
  expect(toggle.getAttribute("aria-expanded")).toBe("false");
  expect(screen.queryByRole("complementary", { name: "Sources" })).toBeNull();
  await user.click(toggle);
  expect(toggle.getAttribute("aria-expanded")).toBe("true");
  const sources = screen.getByRole("complementary", { name: "Sources" });
  // Second fold: entries first, the original text only after a click on an entry.
  const entries = await within(sources).findAllByRole("button", { expanded: false });
  expect(within(sources).queryByText(/The east corridor was sealed in the first month after the tide gates failed/)).toBeNull();
  const entry = entries.find((b) => /Harbor Ledger/.test(b.textContent ?? ""))!;
  await user.click(entry);
  expect(entry.getAttribute("aria-expanded")).toBe("true");
  expect(await within(sources).findByText(/The east corridor was sealed in the first month after the tide gates failed/)).toBeDefined();
  expect(within(sources).getAllByRole("link", { name: "View original" })[0].getAttribute("href")).toMatch(/^https?:/);
  await user.click(entry);
  expect(within(sources).queryByText(/The east corridor was sealed/)).toBeNull();
  await user.click(within(sources).getByRole("button", { name: "Close" }));
  expect(screen.queryByRole("complementary", { name: "Sources" })).toBeNull();
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
  await user.click(await within(panel).findByRole("button", { name: "Show the list" }));
  await user.click(await within(panel).findByRole("button", { name: "Show 2 more" }));
  await user.click(await within(panel).findByText("Tess Arden"));
  await within(await screen.findByRole("log")).findByText(/Take the stairs by the fish market/);
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
  await user.click(screen.getByRole("button", { name: "Attach" }));
  await user.click(await screen.findByRole("menuitem", { name: "Ask someone" }));
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
  // Edit = retract + write anew: the new wording is live, the old one sits under Deleted.
  const row = within(drawer).getByText("Mira noticed you prefer short answers.").closest("li")!;
  await user.click(within(row).getByRole("button", { name: "Edit" }));
  const box = within(row).getByLabelText("New wording");
  await user.clear(box);
  await user.type(box, "Mira noticed you like brief replies.");
  await user.click(within(row).getByRole("button", { name: "Save" }));
  expect(await within(drawer).findByText("Mira noticed you like brief replies.")).toBeDefined();
  await user.click(within(drawer).getByRole("button", { name: "Deleted" }));
  expect(await within(drawer).findByText("Mira noticed you prefer short answers.")).toBeDefined();
  expect(within(drawer).getAllByRole("button", { name: "Restore" }).length).toBe(2);
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

test("library: always reachable from the rail, filtered search, a unit read with its neighbours", async () => {
  const user = userEvent.setup();
  renderApp();
  await screen.findByRole("heading", { name: "Who's waiting" });
  // With a model set up, the library is a choice: the rail opens it and the chats come back.
  await user.click(within(screen.getByRole("navigation", { name: "Sections" })).getByRole("button", { name: "Library" }));
  expect(screen.queryByRole("heading", { name: "Who's waiting" })).toBeNull();
  expect(screen.queryByText(/Not connected\./)).toBeNull();
  const search = screen.getByRole("searchbox", { name: "Search the archive" });
  await user.type(search, "the");
  await user.selectOptions(screen.getByLabelText("Only this kind of text"), "story");
  await user.click(screen.getByRole("button", { name: "Search" }));
  expect(await screen.findByText("East Ward · Night Notes")).toBeDefined();
  expect(screen.queryByText("Harbor Ledger · Flood Years")).toBeNull();
  // A person page opens without a way to talk to a model.
  const rosterRow = screen.getAllByRole("button", { name: "Mira Vale" })[0];
  await user.click(rosterRow);
  const page = await screen.findByRole("complementary", { name: "Roster" });
  expect(within(page).queryByRole("button", { name: "Talk" })).toBeNull();
  await user.click(within(page).getByRole("button", { name: "Close" }));

  const hit = screen.getByText("East Ward · Night Notes").closest("article")!;
  await user.click(within(hit).getByRole("button", { name: "Read in full" }));
  const reader = await screen.findByRole("article", { name: "Passage" });
  expect(await within(reader).findByText("Harbor Ledger · Flood Years")).toBeDefined();
  expect(within(reader).getByText(/It was high summer when the corridor closed/)).toBeDefined();
  expect(within(reader).getByRole("link", { name: "View original" }).getAttribute("href")).toMatch(/^https?:/);
  await user.click(within(reader).getByRole("button", { name: "Back to results" }));
  expect(screen.getByText("East Ward · Night Notes")).toBeDefined();

  // The chats entry goes back to the conversations.
  await user.click(within(screen.getByRole("navigation", { name: "Sections" })).getByRole("button", { name: "Chats" }));
  expect(await screen.findByRole("heading", { name: "Who's waiting" })).toBeDefined();
});

test("first open: name, then the host introduces itself with the coldstart card", async () => {
  const user = userEvent.setup();
  renderApp("first=1");
  // Several steps in a row: each wait is generous, so a loaded machine does not fail it.
  const slow = { timeout: 5000 };
  await user.type(await screen.findByLabelText("What should they call you?", {}, slow), "Rowan");
  await user.click(screen.getByRole("button", { name: "Continue" }));
  await user.click(await screen.findByRole("button", { name: "Keep the default" }, slow));
  expect(await screen.findByRole("heading", { name: "You could start with them" }, slow)).toBeDefined();
  expect(await within(await screen.findByRole("log", {}, slow)).findByText(/Hello, Rowan\./, {}, slow)).toBeDefined();
  expect(screen.getByText("They are all frozen now: they answer when you reach out.")).toBeDefined();
});

test("endpoints: a failed model fetch shows the endpoint's own text and a manual field", async () => {
  const user = userEvent.setup();
  renderApp();
  await screen.findByRole("heading", { name: "Who's waiting" });
  await user.click(screen.getByRole("button", { name: "Menu" }));
  await user.click(within(await screen.findByRole("complementary", { name: "Menu" })).getByRole("button", { name: "Settings" }));
  await user.click(await screen.findByRole("button", { name: "Endpoints" }));
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
