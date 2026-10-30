import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { Api, Passage } from "../api";
import { renderWith } from "../test/render";
import { Library } from "./Library";

function hit(n: number): Passage {
  return {
    citation: { page: `Page ${n}`, revid: 1, span_from: 0, span_to: 9, chunk_id: `c-${n}` },
    template: "card",
    header: `Entry ${n}`,
    text: `${"Long lead-in. ".repeat(30)}The lantern burned all night in room ${n}. ${"Tail. ".repeat(30)}`,
    persons: [{ id: "kal", name: "Kal" }],
    source_url: null,
  };
}

const many = (count: number) => (api: Api): Api => ({
  ...api,
  commands: {
    ...api.commands,
    searchCorpus: async () => ({ status: "ok", data: { hits: Array.from({ length: count }, (_, i) => hit(i + 1)), confidence: "high", understood_as: null } }),
  } as Api["commands"],
});

async function search(user: ReturnType<typeof userEvent.setup>) {
  await user.type(screen.getByRole("searchbox"), "lantern");
  await user.click(screen.getByRole("button", { name: "Search" }));
}

test("a hit is a card: title, type, people, a snippet around the matched word, and a way to read it", async () => {
  const user = userEvent.setup();
  renderWith(<Library />, { patch: many(1) });
  await search(user);
  const card = (await screen.findByText("Entry 1")).closest("article")!;
  expect(within(card).getByText("card")).toBeDefined();
  expect(within(card).getByText("Kal")).toBeDefined();
  const mark = card.querySelector("mark")!;
  expect(mark.textContent).toBe("lantern");
  // The snippet is cut around the hit, not the whole passage.
  expect(card.querySelector(".hit-snippet")!.textContent!.length).toBeLessThan(320);
  expect(within(card).getByRole("button", { name: "Read in full" })).toBeDefined();
});

test("thirty hits at first, thirty more on request", async () => {
  const user = userEvent.setup();
  renderWith(<Library />, { patch: many(65) });
  await search(user);
  await screen.findByText("Entry 1");
  expect(document.querySelectorAll(".hits > li")).toHaveLength(30);
  await user.click(screen.getByRole("button", { name: "Show 30 more" }));
  expect(document.querySelectorAll(".hits > li")).toHaveLength(60);
  await user.click(screen.getByRole("button", { name: "Show 5 more" }));
  expect(document.querySelectorAll(".hits > li")).toHaveLength(65);
  expect(screen.queryByRole("button", { name: /Show \d+ more/ })).toBeNull();
});
