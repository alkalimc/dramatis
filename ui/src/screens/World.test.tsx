import { screen, within } from "@testing-library/react";
import { renderWith } from "../test/render";
import { WorldDrawer } from "./World";

test("the event log lists what happened, with who was part of it", async () => {
  renderWith(<WorldDrawer />);
  const log = await screen.findByRole("region", { name: "On board lately" });
  expect(within(log).getByText("Tea on the deck")).toBeDefined();
  expect(within(log).getByText(/With Mira Vale, /)).toBeDefined();
});
