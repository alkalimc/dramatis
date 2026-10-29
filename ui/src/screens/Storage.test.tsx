import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { vi } from "vitest";
import { renderWith } from "../test/render";
import { StoragePage } from "./Storage";

async function open() {
  const user = userEvent.setup();
  const view = renderWith(<StoragePage />);
  await screen.findByText("Portraits");
  return { user, ...view };
}

const row = (name: string) => screen.getByText(name).closest("li") as HTMLElement;

test("lists every kind with its size, and only the caches can be ticked", async () => {
  await open();
  expect(within(row("Portraits")).getByRole("checkbox")).toBeDefined();
  expect(within(row("Read-aloud audio")).getByRole("checkbox")).toBeDefined();
  for (const fixed of ["Saves", "Corpus file", "Uploads and group pictures"]) {
    expect(within(row(fixed)).queryByRole("checkbox")).toBeNull();
  }
  expect(within(row("Portraits")).getByText("2.2 MB")).toBeDefined();
  expect(screen.getByRole("img", { name: /Disk use: .*Corpus file/ })).toBeDefined();
  expect(screen.getByText(/Pictures in this save: 3/)).toBeDefined();
  expect(screen.getByText(/trimmed to 2.9 GB/)).toBeDefined();
});

test("clearing empties the ticked caches after a yes, and leaves the rest", async () => {
  const { user } = await open();
  const ask = vi.spyOn(window, "confirm").mockReturnValueOnce(false).mockReturnValueOnce(true);
  await user.click(within(row("Portraits")).getByRole("checkbox"));
  const button = screen.getByRole("button", { name: "Clear 5.1 MB" });
  await user.click(button);
  expect(ask).toHaveBeenCalledOnce();
  expect(within(row("Read-aloud audio")).getByText("5.1 MB")).toBeDefined();

  await user.click(screen.getByRole("button", { name: "Clear 5.1 MB" }));
  await waitFor(() => expect(within(row("Read-aloud audio")).getByText("0 KB")).toBeDefined());
  expect(within(row("Portraits")).getByText("2.2 MB")).toBeDefined();
  expect(within(row("Corpus file")).getByText("46.9 MB")).toBeDefined();
  expect(screen.getByRole("button", { name: "Nothing to clear" }).hasAttribute("disabled")).toBe(true);
});
