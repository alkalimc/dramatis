import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import type { Mode } from "../api";
import { renderWith } from "../test/render";
import { ModeSwitch } from "./ModeSwitch";

function Harness({ start, onChange, kind }: { start: Mode; onChange: (m: Mode) => void; kind?: "person" | "group" }) {
  const [mode, setMode] = useState(start);
  return (
    <ModeSwitch
      mode={mode}
      kind={kind}
      label="Mode for Ann"
      onChange={(m) => {
        setMode(m);
        onChange(m);
      }}
    />
  );
}

test("three labelled states, one checked, each with a one-line explanation", () => {
  renderWith(<Harness start="frozen" onChange={() => undefined} />);
  const group = screen.getByRole("group", { name: "Mode for Ann" });
  expect(group).toBeDefined();
  const radios = screen.getAllByRole("radio");
  expect(radios.map((r) => (r as HTMLInputElement).value)).toEqual(["enabled", "frozen", "disabled"]);
  expect((screen.getByRole("radio", { name: "Frozen" }) as HTMLInputElement).checked).toBe(true);
  expect(screen.getByRole("radio", { name: "Enabled" }).getAttribute("aria-describedby")).toBeTruthy();
  expect(screen.getByText("May come to you on their own when something is on their mind.")).toBeDefined();
});

test("clicking and arrow keys move between states", async () => {
  const user = userEvent.setup();
  const seen: Mode[] = [];
  renderWith(<Harness start="frozen" onChange={(m) => seen.push(m)} />);
  await user.click(screen.getByRole("radio", { name: "Enabled" }));
  expect((screen.getByRole("radio", { name: "Enabled" }) as HTMLInputElement).checked).toBe(true);
  await user.click(screen.getByRole("radio", { name: "Disabled" }));
  expect(seen).toEqual(["enabled", "disabled"]);
  // The selected sentence follows the state.
  expect(screen.getAllByText("Out of reach: never offered to anyone, never triggered.").length).toBeGreaterThan(0);
});

test("group switches use the group sentences", () => {
  renderWith(<Harness start="frozen" kind="group" onChange={() => undefined} />);
  expect(screen.getAllByText("Frozen: someone answers only when you speak.").length).toBeGreaterThan(0);
});
