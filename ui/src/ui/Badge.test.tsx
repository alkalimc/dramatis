import { render, screen } from "@testing-library/react";
import { Badge, badgeText } from "./Badge";

test("one and two digits make a plain circle; a third character widens it", () => {
  const { container, rerender } = render(<Badge count={7} />);
  const pill = () => container.querySelector(".badge")!;
  expect(pill().textContent).toBe("7");
  expect(pill().classList.contains("wide")).toBe(false);
  rerender(<Badge count={99} />);
  expect(pill().classList.contains("wide")).toBe(false);
  rerender(<Badge count={100} />);
  expect(pill().textContent).toBe("99+");
  expect(pill().classList.contains("wide")).toBe(true);
  expect(badgeText(1000)).toBe("99+");
});

test("a badge with an action is one button, grey when quiet", () => {
  render(<Badge count={3} quiet label="3 unread" onClick={() => {}} />);
  const b = screen.getByRole("button", { name: "3 unread" });
  expect(b.className).toBe("badge quiet");
});
