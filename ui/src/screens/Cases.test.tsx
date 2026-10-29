import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { renderApp } from "../test/render";

type User = ReturnType<typeof userEvent.setup>;

async function openCases(user: User, query = "", patch?: Parameters<typeof renderApp>[1]) {
  renderApp(query, patch);
  await screen.findByRole("heading", { name: "Who's waiting" });
  await user.click(screen.getByRole("button", { name: "Requests" }));
  return screen.findByRole("complementary");
}

test("empty list says what to do", async () => {
  const user = userEvent.setup();
  const drawer = await openCases(user, "", (api) => ({
    ...api,
    commands: { ...api.commands, tasks: () => Promise.resolve({ status: "ok", data: [] }) },
  }));
  expect(await within(drawer).findByText("Ask a question you want to know the answer to.")).toBeDefined();
});

test("list: pinned questions first, other requests below with a pin action", async () => {
  const user = userEvent.setup();
  const drawer = await openCases(user);
  const pinned = (await within(drawer).findByRole("heading", { name: "Pinned" })).closest("section")!;
  expect(within(pinned).getByText("What really happened in the east corridor?")).toBeDefined();
  // Child requests belong to their case file, not to the list.
  expect(within(drawer).queryByText("When was the east corridor sealed?")).toBeNull();
  const other = within(drawer).getByRole("heading", { name: "Other requests" }).closest("section")!;
  expect(within(other).getByText("Which route is safest after dark?")).toBeDefined();

  await user.click(within(other).getByRole("button", { name: "Pin" }));
  await waitFor(() => {
    const now = within(drawer).getByRole("heading", { name: "Pinned" }).closest("section")!;
    expect(within(now).getByText("Which route is safest after dark?")).toBeDefined();
  });
  expect(within(drawer).queryByRole("heading", { name: "Other requests" })).toBeNull();
});

test("case file: what was found and from whom, with sources, and who is still looking", async () => {
  const user = userEvent.setup();
  const drawer = await openCases(user);
  await user.click(await within(drawer).findByRole("button", { name: "What really happened in the east corridor?" }));
  expect(await within(drawer).findByRole("heading", { name: "What really happened in the east corridor?" })).toBeDefined();

  const found = within(drawer).getByRole("heading", { name: "Found" }).closest("section")!;
  const trail = found.querySelectorAll(".trail > li");
  expect(trail.length).toBe(2);
  // Each line carries the person it came from; scene lines are not part of the finding.
  const answered = trail[0] as HTMLElement;
  expect(within(answered).getByText(/sealed in the \*\*first month\*\* after the gates failed/)).toBeDefined();
  expect(within(answered).getByText("← Mira Vale")).toBeDefined();
  expect(within(answered).queryByText(/runs a finger down the index/)).toBeNull();
  expect(answered.textContent).not.toMatch(/\[#/);
  const sources = answered.querySelector("details")!;
  expect(sources.open).toBe(false);
  expect(within(sources).getByText(/Sources \(2\)/)).toBeDefined();
  // Entries are one line each; the original text shows only after a click on an entry.
  expect(within(sources).queryByText(/sealed in the first month/)).toBeNull();
  await user.click(within(sources).getAllByRole("button", { expanded: false })[0]);
  expect(await within(sources).findByText(/sealed in the first month/)).toBeDefined();
  expect(within(trail[1] as HTMLElement).getByText("Oren Lark is still looking")).toBeDefined();
});

test("case file: the user's conclusion, with the contradicting person's own words quoted once", async () => {
  const user = userEvent.setup();
  const drawer = await openCases(user);
  await user.click(await within(drawer).findByRole("button", { name: "What really happened in the east corridor?" }));
  expect(await within(drawer).findByText("The warning came late, so the corridor closed before people got out.")).toBeDefined();

  const said = within(drawer).getByRole("list", { name: "Said about it" });
  const line = within(said).getByRole("listitem");
  expect(line.textContent).toMatch(/I remember it differently: the evacuation order came late, not the warning\./);
  // The quote is a line that links to his channel; the case file never judges who is right.
  await user.click(within(line).getByRole("button", { name: "Oren Lark" }));
  expect(await screen.findByRole("heading", { name: "Oren Lark" })).toBeDefined();
});

test("pin a question: it opens empty, then a conclusion can be written and changed", async () => {
  const user = userEvent.setup();
  const drawer = await openCases(user);
  await user.type(within(drawer).getByRole("textbox", { name: "Pin a question" }), "Who built the old bridge?{Enter}");

  expect(await within(drawer).findByRole("heading", { name: "Who built the old bridge?" })).toBeDefined();
  expect(within(drawer).getByText("Nothing found yet.")).toBeDefined();
  expect(within(drawer).getByText("No conclusion yet.")).toBeDefined();

  await user.click(within(drawer).getByRole("button", { name: "Write a conclusion" }));
  const save = within(drawer).getByRole("button", { name: "Save conclusion" }) as HTMLButtonElement;
  expect(save.disabled).toBe(true);
  await user.type(within(drawer).getByRole("textbox", { name: "My conclusion" }), "Probably the dock crews.");
  await user.click(save);
  expect(await within(drawer).findByText("Probably the dock crews.")).toBeDefined();
  expect(within(drawer).queryByText("No conclusion yet.")).toBeNull();

  await user.click(within(drawer).getByRole("button", { name: "Change conclusion" }));
  const box = within(drawer).getByRole("textbox", { name: "My conclusion" }) as HTMLTextAreaElement;
  expect(box.value).toBe("Probably the dock crews.");
  await user.clear(box);
  await user.type(box, "The stone came from the quarry crews.");
  await user.click(within(drawer).getByRole("button", { name: "Save conclusion" }));
  expect(await within(drawer).findByText("The stone came from the quarry crews.")).toBeDefined();
  expect(within(drawer).queryByText("Probably the dock crews.")).toBeNull();
});

test("case file: ask again prefills the ask card; unpin returns to the list", async () => {
  const user = userEvent.setup();
  const drawer = await openCases(user);
  await user.click(await within(drawer).findByRole("button", { name: "What really happened in the east corridor?" }));
  await within(drawer).findByRole("heading", { name: "Found" });

  await user.click(within(drawer).getByRole("button", { name: "Ask someone again" }));
  const dialog = await screen.findByRole("dialog", { name: "Ask someone" });
  expect((within(dialog).getByLabelText("What do you want to know?") as HTMLTextAreaElement).value).toBe(
    "What really happened in the east corridor?",
  );
  await user.click(within(dialog).getByRole("button", { name: "Cancel" }));

  await user.click(within(drawer).getByRole("button", { name: "Unpin" }));
  const other = (await within(drawer).findByRole("heading", { name: "Other requests" })).closest("section")!;
  expect(within(other).getByText("What really happened in the east corridor?")).toBeDefined();
});
