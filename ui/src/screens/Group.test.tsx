import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { renderApp } from "../test/render";

async function openGroup(user: ReturnType<typeof userEvent.setup>) {
  renderApp();
  await screen.findByRole("heading", { name: "Who's waiting" });
  await user.click(screen.getByRole("button", { name: "Roster" }));
  const drawer = await screen.findByRole("complementary");
  await user.click(await within(drawer).findByRole("button", { name: "Ward rota" }));
  return screen.findByRole("heading", { name: "Ward rota" });
}

test("group channel: members in the header, speakers in order, scene lines apart", async () => {
  const user = userEvent.setup();
  const title = await openGroup(user);
  const header = title.closest("header")!;
  expect(within(header).getByText(/Members: Oren Lark · Mira Vale · Tess Arden/)).toBeDefined();

  const log = await screen.findByRole("log");
  await within(log).findByText("It did. I'm on nights this week.");
  const speakers = within(log)
    .getAllByRole("article")
    .map((a) => a.getAttribute("aria-label"));
  expect(speakers).toEqual(["Rowan", "Oren Lark", "Tess Arden", "Tess Arden", "Rowan", "Tess Arden"]);

  const scene = within(log).getByText("Tess drops a parcel on the table.");
  expect(scene.className).toBe("scene");
  expect(scene.closest(".bubble")).toBeNull();
  // A thin answer carries the low-confidence tag, same as in a direct channel.
  expect(within(log).getByText("Thin archive")).toBeDefined();
});

test("group mode switch: new groups are frozen, the sentence follows the state", async () => {
  const user = userEvent.setup();
  const title = await openGroup(user);
  const header = title.closest("header")!;
  const frozen = within(header).getByRole("radio", { name: "Frozen" }) as HTMLInputElement;
  expect(frozen.checked).toBe(true);
  expect(header.querySelector(".mode-hint")!.textContent).toBe("Frozen: someone answers only when you speak.");

  await user.click(within(header).getByRole("radio", { name: "Enabled" }));
  await waitFor(() => expect(header.querySelector(".mode-hint")!.textContent).toBe("Enabled: members may speak up on their own."));
  expect((within(header).getByRole("radio", { name: "Enabled" }) as HTMLInputElement).checked).toBe(true);
});

test("enabled group: one person answers and at most one more adds a line like a normal message", async () => {
  const user = userEvent.setup();
  const title = await openGroup(user);
  const header = title.closest("header")!;
  const log = await screen.findByRole("log");
  await within(log).findByText("It did. I'm on nights this week.");

  await user.click(within(header).getByRole("radio", { name: "Enabled" }));
  await waitFor(() => expect(header.querySelector(".mode-hint")!.textContent).toMatch(/^Enabled/));
  const before = within(log).getAllByRole("article").length;

  await user.type(screen.getByRole("textbox", { name: "Message" }), "Tess, what do you know about the tide?{Enter}");
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
  await user.type(screen.getByRole("textbox", { name: "Message" }), "Tess, what do you know about the tide?{Enter}");
  await waitFor(() => expect(within(log).getAllByRole("article").length).toBe(before + 2));
  expect(within(log).queryByText(/If I may add/)).toBeNull();
});

test("group dialog: topic may stay empty and says so; no member cap; frozen by default", async () => {
  const user = userEvent.setup();
  renderApp();
  await screen.findByRole("heading", { name: "Who's waiting" });
  await user.click(screen.getAllByRole("button", { name: /Bring people in/ })[0]);
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
  const header = (await screen.findByRole("heading", { name: "Group" })).closest("header")!;
  expect((within(header).getByRole("radio", { name: "Frozen" }) as HTMLInputElement).checked).toBe(true);
});

test("group menu: deleting asks first and removes the group", async () => {
  const user = userEvent.setup();
  const title = await openGroup(user);
  const header = title.closest("header")!;
  const confirm = vi.spyOn(window, "confirm").mockReturnValueOnce(false).mockReturnValueOnce(true);
  await user.click(within(header).getByRole("button", { name: "More" }));
  await user.click(within(header).getByRole("button", { name: "Delete group" }));
  expect(screen.getByRole("heading", { name: "Ward rota" })).toBeDefined();
  await user.click(within(header).getByRole("button", { name: "Delete group" }));
  await waitFor(() => expect(screen.queryByRole("heading", { name: "Ward rota" })).toBeNull());
  expect(confirm).toHaveBeenCalledTimes(2);
});
