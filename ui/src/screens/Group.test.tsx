import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { renderApp } from "../test/render";

const TITLE = "Oren Lark, Mira Vale, Tess Arden";

async function openGroup(user: ReturnType<typeof userEvent.setup>) {
  renderApp();
  await screen.findByRole("heading", { name: "Who's waiting" });
  await user.click(screen.getByRole("button", { name: "Roster" }));
  const drawer = await screen.findByRole("complementary", { name: "Roster" });
  await user.click(await within(drawer).findByRole("button", { name: TITLE }));
  return screen.findByRole("heading", { name: TITLE });
}

/** The group's info panel, opened from the header icon. */
async function openInfo(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole("button", { name: "Group info" }));
  return screen.findByRole("complementary", { name: "Group info" });
}

test("group channel: members in the header, speakers in order, scene lines apart", async () => {
  const user = userEvent.setup();
  const title = await openGroup(user);
  const header = title.closest("header")!;
  // Named after its members, with how many are in it.
  expect(within(header).getByText("3 members")).toBeDefined();

  const log = await screen.findByRole("log");
  await within(log).findByText("It did. I'm on nights this week.");
  const speakers = within(log)
    .getAllByRole("article")
    .map((a) => a.getAttribute("aria-label"));
  expect(speakers).toEqual(["Rowan", "Oren Lark", "Tess Arden", "Tess Arden", "Rowan", "Tess Arden"]);

  const scene = within(log).getByText("Tess drops a parcel on the table.");
  expect(scene.className).toBe("scene");
  expect(scene.closest(".bubble")).not.toBeNull();
  expect(scene.closest(".speech")).toBeNull();
  // A thin answer carries the low-confidence tag, same as in a direct channel.
  expect(within(log).getByText("Thin archive")).toBeDefined();
});

test("group mode switch lives in the info panel: new groups are frozen, the sentence follows the state", async () => {
  const user = userEvent.setup();
  await openGroup(user);
  const panel = await openInfo(user);
  const frozen = within(panel).getByRole("radio", { name: "Frozen" }) as HTMLInputElement;
  expect(frozen.checked).toBe(true);
  expect(panel.querySelector(".mode-hint")).not.toBeNull();
  expect(within(panel).getByText("Frozen: someone answers only when you speak.")).toBeDefined();

  await user.click(within(panel).getByRole("radio", { name: "Enabled" }));
  await waitFor(() => expect((within(panel).getByRole("radio", { name: "Enabled" }) as HTMLInputElement).checked).toBe(true));
});

test("the same header icon closes the info panel again", async () => {
  const user = userEvent.setup();
  await openGroup(user);
  await openInfo(user);
  const icon = screen.getByRole("button", { name: "Group info" });
  expect(icon.getAttribute("aria-pressed")).toBe("true");
  await user.click(icon);
  expect(screen.queryByRole("complementary", { name: "Group info" })).toBeNull();
});

test("a group can be renamed in the info panel, and goes back to its members' names when emptied", async () => {
  const user = userEvent.setup();
  await openGroup(user);
  const panel = await openInfo(user);
  await user.click(within(panel).getByRole("button", { name: "Rename" }));
  await user.type(within(panel).getByRole("textbox", { name: "Rename" }), "Night shift");
  await user.click(within(panel).getByRole("button", { name: "Save" }));
  await waitFor(() => expect(screen.getAllByRole("heading", { name: "Night shift" }).length).toBe(2));
  await user.click(within(panel).getByRole("button", { name: "Rename" }));
  await user.clear(within(panel).getByRole("textbox", { name: "Rename" }));
  await user.click(within(panel).getByRole("button", { name: "Save" }));
  await waitFor(() => expect(screen.getAllByRole("heading", { name: TITLE }).length).toBeGreaterThan(0));
});

test("the info panel shows what a group is about unless that is already its name", async () => {
  const user = userEvent.setup();
  await openGroup(user);
  const panel = await openInfo(user);
  // Named after its members: the topic is the description.
  expect(within(panel).getByText("About")).toBeDefined();
  await user.click(within(panel).getByRole("button", { name: "Rename" }));
  await user.type(within(panel).getByRole("textbox", { name: "Rename" }), "Ward rota");
  await user.click(within(panel).getByRole("button", { name: "Save" }));
  await waitFor(() => expect(screen.getAllByRole("heading", { name: "Ward rota" }).length).toBeGreaterThan(0));
  // Named by its topic: said once, as the title.
  expect(within(panel).queryByText("About")).toBeNull();
});

test("enabled group: one person answers and at most one more adds a line like a normal message", async () => {
  const user = userEvent.setup();
  await openGroup(user);
  const panel = await openInfo(user);
  const log = await screen.findByRole("log");
  await within(log).findByText("It did. I'm on nights this week.");

  await user.click(within(panel).getByRole("radio", { name: "Enabled" }));
  await waitFor(() => expect((within(panel).getByRole("radio", { name: "Enabled" }) as HTMLInputElement).checked).toBe(true));
  const before = within(log).getAllByRole("article").length;

  await user.type(screen.getByRole("textbox", { name: "Message" }), "Tess, what do you know about the tide?{Meta>}{Enter}{/Meta}");
  const interjection = await within(log).findByText("If I may add: I've seen something about that too.");
  expect(interjection.closest("article")!.className).toContain("from-other");
  const added = within(log).getAllByRole("article").slice(before);
  // The user's line, the addressed person, then one interjection from somebody else.
  expect(added.map((a) => a.getAttribute("aria-label"))).toEqual(["Rowan", "Tess Arden", "Mira Vale"]);
});

test("frozen group: only the addressed person replies", async () => {
  const user = userEvent.setup();
  await openGroup(user);
  const log = await screen.findByRole("log");
  await within(log).findByText("It did. I'm on nights this week.");
  const before = within(log).getAllByRole("article").length;
  await user.type(screen.getByRole("textbox", { name: "Message" }), "Tess, what do you know about the tide?{Meta>}{Enter}{/Meta}");
  await waitFor(() => expect(within(log).getAllByRole("article").length).toBe(before + 2));
  expect(within(log).queryByText(/If I may add/)).toBeNull();
});

test("group dialog: topic may stay empty and says so; no member cap; frozen by default", async () => {
  const user = userEvent.setup();
  renderApp();
  await screen.findByRole("heading", { name: "Who's waiting" });
  await user.click(screen.getByRole("button", { name: "Attach" }));
  await user.click(await screen.findByRole("menuitem", { name: "Bring people in" }));
  // First the group's name, then who is in it.
  await user.click(await screen.findByRole("button", { name: "Next" }));
  const dialog = await screen.findByRole("dialog", { name: "Bring people in" });
  expect(within(dialog).getByText("Leave empty for small talk.")).toBeDefined();
  expect(within(dialog).getByText(/Frozen: someone answers only when you speak/)).toBeDefined();
  const start = within(dialog).getByRole("button", { name: "Start" }) as HTMLButtonElement;
  expect(start.disabled).toBe(true);

  const search = within(dialog).getByRole("searchbox");
  for (const name of ["Mira", "Oren", "Tess", "Juno"]) {
    await user.clear(search);
    await user.type(search, name);
    await user.click(await within(dialog).findByRole("button", { name: new RegExp(`^Add ${name}`) }));
  }
  expect(within(dialog).getAllByRole("button", { name: /^Remove / }).length).toBe(4);
  expect(start.disabled).toBe(false);

  await user.click(start);
  // Topic left empty: the group opens untitled and frozen.
  await screen.findByRole("heading", { name: /Mira Vale, Oren Lark|Mira Vale/ });
  const panel = await openInfo(user);
  expect((within(panel).getByRole("radio", { name: "Frozen" }) as HTMLInputElement).checked).toBe(true);
});

test("group menu: deleting asks first and removes the group", async () => {
  const user = userEvent.setup();
  const title = await openGroup(user);
  const header = title.closest("header")!;
  const confirm = vi.spyOn(window, "confirm").mockReturnValueOnce(false).mockReturnValueOnce(true);
  await user.click(within(header).getByRole("button", { name: "More" }));
  await user.click(await screen.findByRole("menuitem", { name: "Delete group" }));
  expect(screen.getByRole("heading", { name: TITLE })).toBeDefined();
  await user.click(within(header).getByRole("button", { name: "More" }));
  await user.click(await screen.findByRole("menuitem", { name: "Delete group" }));
  await waitFor(() => expect(screen.queryByRole("heading", { name: TITLE })).toBeNull());
  expect(confirm).toHaveBeenCalledTimes(2);
});

test("a member is taken out after asking, the title follows, and the group says so", async () => {
  const user = userEvent.setup();
  await openGroup(user);
  const panel = await openInfo(user);
  const confirm = vi.spyOn(window, "confirm").mockReturnValueOnce(false).mockReturnValueOnce(true);
  await user.click(within(panel).getByRole("button", { name: "Take Tess Arden out" }));
  expect(within(panel).getByText("3 members")).toBeDefined();
  await user.click(within(panel).getByRole("button", { name: "Take Tess Arden out" }));
  expect(confirm).toHaveBeenCalledTimes(2);
  await waitFor(() => expect(within(panel).getByText("2 members")).toBeDefined());
  // No custom name: the title is the remaining members' names, in the list and the header.
  await waitFor(() => expect(screen.getAllByRole("heading", { name: "Oren Lark, Mira Vale" }).length).toBeGreaterThan(0));
  expect(await within(await screen.findByRole("log")).findByText("Tess Arden leaves the conversation.")).toBeDefined();
  expect(within(panel).queryByRole("button", { name: "Take Tess Arden out" })).toBeNull();
});

test("people are added from a picker that leaves out the members, and the title follows", async () => {
  const user = userEvent.setup();
  await openGroup(user);
  const panel = await openInfo(user);
  await user.click(within(panel).getByRole("button", { name: "Add people" }));
  const dialog = await screen.findByRole("dialog", { name: "Add people to this group" });
  const search = within(dialog).getByRole("searchbox");
  await user.type(search, "Tess");
  expect(within(dialog).queryByRole("button", { name: "Tess Arden" })).toBeNull();
  await user.clear(search);
  await user.type(search, "Juno");
  await user.click(await within(dialog).findByRole("button", { name: "Juno Reyes" }));
  await waitFor(() => expect(within(panel).getByText("4 members")).toBeDefined());
  await waitFor(() => expect(screen.getAllByRole("heading", { name: "Oren Lark, Mira Vale (4 in all)" }).length).toBeGreaterThan(0));
  expect(await within(await screen.findByRole("log")).findByText("Juno Reyes joins the conversation.")).toBeDefined();
});

test("a colleague's invitation is a button under the message, and goes once the person is in", async () => {
  const user = userEvent.setup();
  await openGroup(user);
  const panel = await openInfo(user);
  const log = await screen.findByRole("log");
  const invite = await within(log).findByRole("button", { name: "Invite Juno Reyes to join" });
  await user.click(invite);
  await waitFor(() => expect(within(panel).getByText("4 members")).toBeDefined());
  await waitFor(() => expect(within(log).queryByRole("button", { name: "Invite Juno Reyes to join" })).toBeNull());
});

test("a group made from the dialog has an info panel before anyone has spoken in it", async () => {
  const user = userEvent.setup();
  // As in the engine, a group nobody has spoken in is not in the list yet.
  const created = new Set<string>();
  renderApp("", (api) => ({
    ...api,
    commands: {
      ...api.commands,
      createGroup: async (m, topic) => {
        const r = await api.commands.createGroup(m, topic);
        if (r.status === "ok") created.add(r.data);
        return r;
      },
      channels: async () => {
        const r = await api.commands.channels();
        return r.status === "ok" ? { ...r, data: r.data.filter((c) => !created.has(c.id)) } : r;
      },
    },
  }));
  await screen.findByRole("heading", { name: "Who's waiting" });
  await user.click(screen.getByRole("button", { name: "Attach" }));
  await user.click(await screen.findByRole("menuitem", { name: "Bring people in" }));
  await user.click(await screen.findByRole("button", { name: "Next" }));
  const dialog = await screen.findByRole("dialog", { name: "Bring people in" });
  await user.type(within(dialog).getByRole("searchbox"), "Mira");
  await user.click(await within(dialog).findByRole("button", { name: /^Add Mira/ }));
  await user.click(within(dialog).getByRole("button", { name: "Start" }));
  const panel = await openInfo(user);
  // Not stuck on "Loading": the members and the mode are there.
  expect(within(panel).queryByText("Loading")).toBeNull();
  expect(within(panel).getByRole("heading", { name: "Members" })).toBeDefined();
  await user.click(within(panel).getByRole("radio", { name: "Enabled" }));
  await waitFor(() => expect((within(panel).getByRole("radio", { name: "Enabled" }) as HTMLInputElement).checked).toBe(true));
});
