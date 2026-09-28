import { render, screen } from "@testing-library/react";
import { App } from "./App";

test("renders the form the engine reports", async () => {
  const load = async () => ({
    status: "ok" as const,
    data: { form: "library" as const, corpus: { state: "missing" as const }, needs_setup: true },
  });
  render(<App load={load} />);
  expect(await screen.findByText("library")).toBeDefined();
});
