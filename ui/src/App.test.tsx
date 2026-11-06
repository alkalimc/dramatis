import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
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
  // The block sits inside the message's bubble, not beside it.
  expect(sources.closest(".bubble")).not.toBeNull();
  await user.click(within(sources).getByRole("button", { name: "Fold the sources" }));
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

test("roster: bands, no numbers on rows, disabled collapsed", async () => {
  const user = userEvent.setup();
  renderApp();
  await screen.findByRole("heading", { name: "Who's waiting" });
  await user.click(screen.getByRole("button", { name: "Roster" }));
  // The roster is a page of its own, not a panel beside the conversation.
  expect(screen.queryByRole("complementary")).toBeNull();
  const page = (await screen.findByRole("heading", { name: "Roster" })).closest(".page")! as HTMLElement;
  expect(await within(page).findByRole("heading", { name: "Close" })).toBeDefined();
  const disabled = within(page).getByText("Disabled (1)").closest("details")!;
  expect(disabled.open).toBe(false);
  for (const row of page.querySelectorAll(".roster-row")) {
    expect(row.textContent).not.toMatch(/\b\d{2,3}\b/);
  }
  // A person opens in the right panel beside the page.
  await user.click(within(page).getByRole("button", { name: "Mira Vale" }));
  const drawer = await screen.findByRole("complementary");
  expect(await within(drawer).findByText("Trust 137")).toBeDefined();
  expect(within(drawer).getByRole("group", { name: "Read-aloud voice" })).toBeDefined();
  await user.click(within(drawer).getByRole("button", { name: /Back to roster/ }));
  expect(screen.queryByRole("complementary")).toBeNull();
  expect(screen.getByRole("heading", { name: "Roster" })).toBeDefined();
});

test("world page: visibility on every row, hurt shows the quote and its effect", async () => {
  const user = userEvent.setup();
  renderApp();
  await screen.findByRole("heading", { name: "Who's waiting" });
  await user.click(screen.getByRole("button", { name: "World" }));
  expect(screen.queryByRole("complementary")).toBeNull();
  const drawer = (await screen.findByRole("heading", { name: "What happened" })).closest(".page")! as HTMLElement;
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
  expect(await screen.findByRole("region", { name: "Models and endpoints" })).toBeDefined();
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

  // The library holds no roster of its own: a person page opened from the roster drawer talks, the chat opens and the library closes.
  await user.click(within(screen.getByRole("navigation", { name: "Sections" })).getByRole("button", { name: "Library" }));
  expect(screen.queryByRole("button", { name: "Mira Vale" })).toBeNull();
  await user.click(within(screen.getByRole("navigation", { name: "Sections" })).getByRole("button", { name: "Roster" }));
  await user.click(await screen.findByRole("button", { name: "Mira Vale" }));
  const page = await screen.findByRole("complementary", { name: "Roster" });
  await user.click(within(page).getByRole("button", { name: "Talk" }));
  expect(await screen.findByRole("textbox", { name: /Message/ })).toBeDefined();
  expect(screen.queryByRole("searchbox", { name: "Search the archive" })).toBeNull();
  expect(screen.queryByRole("complementary", { name: "Roster" })).toBeNull();
});

test("first open: name, then the host introduces itself with the coldstart card", async () => {
  const user = userEvent.setup();
  renderApp("first=1");
  // Several steps in a row: each wait is generous, so a loaded machine does not fail it.
  const slow = { timeout: 5000 };
  await user.type(await screen.findByLabelText("What should they call you?", {}, slow), "Rowan");
  await user.click(screen.getByRole("button", { name: "Continue" }));
  await user.click(await screen.findByRole("button", { name: "Keep the default" }, slow));
  await user.click(await screen.findByRole("button", { name: "Not now" }, slow));
  expect(await screen.findByRole("heading", { name: "You could start with them" }, slow)).toBeDefined();
  expect(await within(await screen.findByRole("log", {}, slow)).findByText(/Hello, Rowan\./, {}, slow)).toBeDefined();
  expect(screen.getByText("They are all frozen now: they answer when you reach out.")).toBeDefined();
});

test("endpoints: a failed check shows the endpoint's own text, a manual field, and the key can be stored from the same editor", async () => {
  const user = userEvent.setup();
  renderApp();
  await screen.findByRole("heading", { name: "Who's waiting" });
  await user.click(screen.getByRole("button", { name: "Menu" }));
  await user.click(within(await screen.findByRole("complementary", { name: "Menu" })).getByRole("button", { name: "Settings" }));
  await user.click(await screen.findByRole("button", { name: "Models and endpoints" }));
  const card = (await screen.findByRole("heading", { name: "deepseek" })).closest("li")!;
  expect(within(card).getByText("No key stored")).toBeDefined();
  await user.click(within(card).getByRole("button", { name: "Edit deepseek" }));
  // The model list is checked from the same editor; without a key the endpoint refuses.
  await user.click(within(card).getByRole("button", { name: "Refresh models" }));
  expect(await within(card).findByText(/401 Unauthorized/)).toBeDefined();
  expect(within(card).getByLabelText("Model for chat")).toBeDefined();
  // Storing a key leaves only the notice; the key is never shown again.
  await user.type(within(card).getByLabelText("API key"), "sk-test");
  await user.click(within(card).getByRole("button", { name: "Refresh models" }));
  expect(await within(card).findByText("Stored in keychain")).toBeDefined();
  expect(within(card).queryByDisplayValue("sk-test")).toBeNull();
  expect(await within(card).findByText("Models found: 2")).toBeDefined();
});

test("the host's name and avatar in its channel open its page, which has no trust or mode and leads to Talk", async () => {
  const user = userEvent.setup();
  renderApp();
  const log = await screen.findByRole("log");
  // Each local day in the log starts with a date pill.
  expect(within(log).getAllByRole("separator").length).toBeGreaterThan(0);
  await within(log).findByText(/Host ▸ finding who knows/);
  await user.click(within(log).getAllByRole("button", { name: "Open Host's page" })[0]);
  const card = await screen.findByRole("dialog", { name: "Host" });
  expect(within(card).getByText(/The assistant of this place/)).toBeDefined();
  expect(within(card).getByRole("heading", { name: "What it does" })).toBeDefined();
  expect(within(card).queryByText(/Trust/)).toBeNull();
  expect(within(card).queryByRole("radio")).toBeNull();
  // The happenings setting is one click away, on the quota page, and the card gives way to it.
  await user.click(within(card).getByRole("button", { name: "Happenings on board" }));
  expect(await screen.findByRole("region", { name: "Quota and intensity" })).toBeDefined();
  expect(screen.queryByRole("dialog", { name: "Host" })).toBeNull();
});

const rail = () => within(screen.getByRole("navigation", { name: "Sections" }));
const pressed = (name: string) => rail().getByRole("button", { name }).getAttribute("aria-pressed");

test("in the library, Roster and World are highlighted and fill the page like Chats and Library", async () => {
  const user = userEvent.setup();
  renderApp("form=library");
  expect(await screen.findByText(/Not connected\./)).toBeDefined();
  expect(pressed("Library")).toBe("true");
  await user.click(rail().getByRole("button", { name: "Roster" }));
  expect(pressed("Roster")).toBe("true");
  expect(pressed("Library")).toBe("false");
  expect(await screen.findByRole("heading", { name: "Roster" })).toBeDefined();
  expect(screen.queryByRole("searchbox", { name: "Search the archive" })).toBeNull();
  expect(screen.queryByRole("complementary")).toBeNull();
  await user.click(rail().getByRole("button", { name: "World" }));
  expect(pressed("World")).toBe("true");
  expect(pressed("Roster")).toBe("false");
  expect(await screen.findByRole("heading", { name: "What happened" })).toBeDefined();
  // Pressing the lit one again goes back to where the library is.
  await user.click(rail().getByRole("button", { name: "World" }));
  expect(pressed("Library")).toBe("true");
  expect(await screen.findByRole("searchbox", { name: "Search the archive" })).toBeDefined();
});

test("the four sections are peers: exactly one is lit, and a chat opened from a page ends it", async () => {
  const user = userEvent.setup();
  renderApp();
  await screen.findByRole("heading", { name: "Who's waiting" });
  const lit = () => ["Chats", "Library", "Roster", "World"].filter((n) => pressed(n) === "true");
  expect(lit()).toEqual(["Chats"]);
  for (const name of ["Library", "Roster", "World", "Library", "Chats"]) {
    await user.click(rail().getByRole("button", { name }));
    expect(lit()).toEqual([name]);
  }
  await user.click(rail().getByRole("button", { name: "Roster" }));
  await user.click(await screen.findByRole("button", { name: "Rowan, Oren Lark (4 in all)" }));
  expect(lit()).toEqual(["Chats"]);
});

test("when the area beside the chat list gets too narrow for the panel, the panel closes and the chat stays", async () => {
  const user = userEvent.setup();
  const watchers = new Set<(entries: { contentRect: { width: number } }[]) => void>();
  class FakeObserver {
    constructor(private readonly f: (entries: { contentRect: { width: number } }[]) => void) {
      watchers.add(f);
    }
    observe() {}
    disconnect() {
      watchers.delete(this.f);
    }
  }
  const report = (width: number) => act(() => watchers.forEach((f) => f([{ contentRect: { width } }])));
  vi.stubGlobal("ResizeObserver", FakeObserver);
  try {
    renderApp();
    await screen.findByRole("heading", { name: "Who's waiting" });
    // Roomy: 56 rems of 15px is where the panel stops docking.
    report(1000);
    await user.click(screen.getByRole("button", { name: "Profile" }));
    expect(await screen.findByRole("complementary")).toBeDefined();
    report(900);
    expect(screen.getByRole("complementary")).toBeDefined();
    report(700);
    await waitFor(() => expect(screen.queryByRole("complementary")).toBeNull());
    // The chat and its list are untouched by this step; folding the list is the stylesheet's, further down.
    expect(screen.getByRole("navigation", { name: "Chats" })).toBeDefined();
    expect(screen.getByRole("heading", { name: "Who's waiting" })).toBeDefined();
    // Opened again while narrow, the panel stays until asked to go (it floats over the chat).
    await user.click(screen.getByRole("button", { name: "Profile" }));
    expect(await screen.findByRole("complementary")).toBeDefined();
    report(650);
    expect(screen.getByRole("complementary")).toBeDefined();
  } finally {
    vi.unstubAllGlobals();
  }
});
