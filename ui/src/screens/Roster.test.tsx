import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { renderWith } from "../test/render";
import { RosterList } from "./Roster";

async function open() {
  const view = renderWith(<RosterList />);
  await view.actions.refreshRoster();
  await screen.findByText("Silas Crane");
  return { user: userEvent.setup(), ...view };
}

const chip = (name: string) => screen.getByRole("button", { name });
const seen = (name: string) => screen.queryByText(name) !== null;

test("mode chips narrow the list; all brings everyone back", async () => {
  const { user } = await open();
  await user.click(chip("Enabled"));
  expect(seen("Mira Vale")).toBe(true);
  expect(seen("Oren Lark")).toBe(false);
  expect(seen("Silas Crane")).toBe(false);
  await user.click(chip("Disabled"));
  expect(seen("Silas Crane")).toBe(true);
  expect(seen("Mira Vale")).toBe(false);
  await user.click(screen.getAllByRole("button", { name: "All" })[0]);
  expect(seen("Mira Vale") && seen("Oren Lark") && seen("Silas Crane")).toBe(true);
});

test("talked to and never talked split by contact, even for a disabled person", async () => {
  const { user } = await open();
  await user.click(chip("Never talked"));
  expect(seen("Kael Moss")).toBe(true);
  expect(seen("Mira Vale")).toBe(false);
  expect(seen("Silas Crane")).toBe(false);
  await user.click(chip("Talked to"));
  expect(seen("Mira Vale")).toBe(true);
  expect(seen("Silas Crane")).toBe(true);
  expect(seen("Kael Moss")).toBe(false);
});

test("filters combine with each other, say so when nothing is left, and are kept", async () => {
  const { user, store, unmount } = await open();
  await user.click(chip("Disabled"));
  await user.click(chip("Never talked"));
  expect(await screen.findByText("Nobody matches these filters.")).toBeDefined();
  expect(store.getState().rosterFilter).toEqual({ mode: "disabled", talk: "never", persona: false });
  unmount();
  // The choice lives in the store, so the drawer opens as it was left.
  renderWith(<RosterList />, { store });
  expect(chip("Disabled").getAttribute("aria-pressed")).toBe("true");
  expect(screen.getByText("Nobody matches these filters.")).toBeDefined();
});

test("has a persona keeps only people who can be talked to", async () => {
  const { user, store } = await open();
  await user.click(chip("Has a persona"));
  expect(store.getState().rosterFilter.persona).toBe(true);
  expect(seen("Mira Vale")).toBe(true);
  // A row without a persona drops out.
  store.dispatch({
    type: "roster",
    roster: {
      total: 2,
      enabled: 0,
      groups: [
        {
          band: "acquainted",
          rows: [
            { person: { id: "a", name: "Has One" }, mode: "frozen", has_persona: true, talked: true, recent: null, companions: [] },
            { person: { id: "b", name: "Has None" }, mode: "frozen", has_persona: false, talked: true, recent: null, companions: [] },
          ],
        },
      ],
    },
  });
  await waitFor(() => expect(seen("Has None")).toBe(false));
  expect(seen("Has One")).toBe(true);
});

test("the filters also narrow what a topic search found", async () => {
  const { user, container } = await open();
  await user.type(screen.getByRole("searchbox"), "zzzz");
  await user.click(screen.getByRole("button", { name: "Find" }));
  // With no match the stand-in offers Mira, who is enabled and has been talked to.
  await waitFor(() => expect(container.querySelector(".candidates")).not.toBeNull());
  await user.click(chip("Never talked"));
  expect(container.querySelector(".candidates")).toBeNull();
  await user.click(chip("Talked to"));
  expect(container.querySelector(".candidates")).not.toBeNull();
});
