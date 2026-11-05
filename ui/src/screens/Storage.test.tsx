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

test("where files are kept: the folder in use, a move that asks first, and the way back to the default", async () => {
  const { user, api } = await open();
  const move = vi.spyOn(api.commands, "setHomeLocation");
  const ask = vi.spyOn(window, "confirm").mockReturnValueOnce(false).mockReturnValue(true);
  expect(screen.getByText("/Users/demo/.dramatis")).toBeDefined();
  expect(screen.getByText("The default folder.")).toBeDefined();
  expect(screen.queryByRole("button", { name: "Use the default folder" })).toBeNull();

  // Without a folder dialog the path is typed; a move needs one.
  const use = screen.getByRole("button", { name: "Use this folder" }) as HTMLButtonElement;
  expect(use.disabled).toBe(true);
  await user.click(screen.getByRole("button", { name: "Choose a folder" }));
  expect(await screen.findByText("This build has no folder dialog. Type the path.")).toBeDefined();
  await user.type(screen.getByLabelText("Folder path"), "/Volumes/Data/dramatis");
  await user.click(use);
  expect(move).not.toHaveBeenCalled();
  expect(ask).toHaveBeenCalledWith("The app restarts to use /Volumes/Data/dramatis. Continue?");

  // Copying is the default; a folder that already holds files is told apart from a bad path.
  await user.clear(screen.getByLabelText("Folder path"));
  await user.type(screen.getByLabelText("Folder path"), "/Volumes/taken");
  await user.click(use);
  expect(await screen.findByText(/already holds files from this app/)).toBeDefined();
  expect(move).toHaveBeenLastCalledWith("/Volumes/taken", "copy");

  await user.clear(screen.getByLabelText("Folder path"));
  await user.type(screen.getByLabelText("Folder path"), "/Volumes/Data/dramatis");
  await user.click(screen.getByRole("radio", { name: /^Use the new folder as it is/ }));
  await user.click(use);
  expect(await screen.findByText("Restarting…")).toBeDefined();
  expect(move).toHaveBeenLastCalledWith("/Volumes/Data/dramatis", "empty");
});

test("a folder fixed from outside the app is shown and cannot be changed", async () => {
  renderWith(<StoragePage />, { query: "home=fixed" });
  expect(await screen.findByText(/Fixed from outside the app/)).toBeDefined();
  expect(screen.queryByLabelText("Folder path")).toBeNull();
});
