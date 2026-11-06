import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { renderWith } from "../test/render";
import { PeopleSearch, PickedChips, personEntry, type Entry } from "./PeopleList";

const people = [
  { id: "a", name: "Ann" },
  { id: "b", name: "Bea" },
  { id: "c", name: "Cy" },
];

function Harness({ onPick, enter, entries }: { onPick: (e: Entry) => void; enter?: "first" | (() => void); entries?: Entry[] }) {
  return <PeopleSearch value="" onChange={() => undefined} placeholder="Find" listLabel="People" entries={entries ?? people.map((p) => personEntry(p, { secondary: `about ${p.name}` }))} empty="Nobody" enter={enter} onPick={onPick} />;
}

test("a row shows picture, name and second line, and a press takes the person", async () => {
  const user = userEvent.setup();
  const pick = vi.fn();
  renderWith(<Harness onPick={pick} />);
  expect(screen.getByText("about Bea")).toBeDefined();
  await user.click(screen.getByRole("button", { name: /Bea/ }));
  expect(pick).toHaveBeenCalledWith(expect.objectContaining({ key: "b" }));
});

test("Enter in the field takes the first enabled row; Down goes to the rows and Up and Down walk them", async () => {
  const user = userEvent.setup();
  const pick = vi.fn();
  const entries = [personEntry(people[0], { disabled: true }), personEntry(people[1]), personEntry(people[2])];
  renderWith(<Harness onPick={pick} enter="first" entries={entries} />);
  await user.click(screen.getByRole("searchbox", { name: "Find" }));
  await user.keyboard("{ArrowDown}");
  expect(document.activeElement).toBe(screen.getByRole("button", { name: /Bea/ }));
  await user.keyboard("{ArrowDown}");
  expect(document.activeElement).toBe(screen.getByRole("button", { name: /Cy/ }));
  await user.keyboard("{ArrowUp}");
  expect(document.activeElement).toBe(screen.getByRole("button", { name: /Bea/ }));
  await user.click(screen.getByRole("searchbox", { name: "Find" }));
  await user.keyboard("{Enter}");
  expect(pick).toHaveBeenCalledWith(expect.objectContaining({ key: "b" }));
});

test("Enter runs a given function instead of taking a row, and an empty list says so", async () => {
  const user = userEvent.setup();
  const pick = vi.fn();
  const search = vi.fn();
  renderWith(<Harness onPick={pick} enter={search} />);
  await user.click(screen.getByRole("searchbox", { name: "Find" }));
  await user.keyboard("{Enter}");
  expect(search).toHaveBeenCalledOnce();
  expect(pick).not.toHaveBeenCalled();
});

test("chosen people are chips that take themselves out again", async () => {
  const user = userEvent.setup();
  const remove = vi.fn();
  renderWith(<PickedChips people={people.slice(0, 2)} remove={(p) => `Remove ${p.name}`} onRemove={remove} />);
  await user.click(screen.getByRole("button", { name: "Remove Bea" }));
  expect(remove).toHaveBeenCalledWith(people[1]);
});
