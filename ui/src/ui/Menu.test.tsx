import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { Menu } from "./Menu";

function open(onClose = vi.fn(), onSelect = vi.fn()) {
  const items = [
    { label: "One", onSelect },
    { label: "Two", onSelect: vi.fn() },
    { label: "Three", onSelect: vi.fn(), separated: true },
  ];
  render(<Menu at={{ x: 10, y: 10 }} items={items} label="Actions" onClose={onClose} />);
  return { onClose, onSelect };
}

describe("Menu", () => {
  it("is a labelled menu of menuitems and focuses the first", () => {
    open();
    expect(screen.getByRole("menu", { name: "Actions" })).toBeTruthy();
    const items = screen.getAllByRole("menuitem");
    expect(items.map((i) => i.textContent)).toEqual(["One", "Two", "Three"]);
    expect(document.activeElement).toBe(items[0]);
  });

  it("moves focus with arrows, wrapping, and Home and End", () => {
    open();
    const menu = screen.getByRole("menu");
    const [one, two, three] = screen.getAllByRole("menuitem");
    fireEvent.keyDown(menu, { key: "ArrowDown" });
    expect(document.activeElement).toBe(two);
    fireEvent.keyDown(menu, { key: "End" });
    expect(document.activeElement).toBe(three);
    fireEvent.keyDown(menu, { key: "ArrowDown" });
    expect(document.activeElement).toBe(one);
    fireEvent.keyDown(menu, { key: "ArrowUp" });
    expect(document.activeElement).toBe(three);
    fireEvent.keyDown(menu, { key: "Home" });
    expect(document.activeElement).toBe(one);
  });

  it("closes on Escape and runs the chosen item", () => {
    const { onClose, onSelect } = open();
    fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("menuitem", { name: "One" }));
    expect(onSelect).toHaveBeenCalledTimes(1);
  });
});
