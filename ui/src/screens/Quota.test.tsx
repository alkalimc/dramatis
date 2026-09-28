import { fireEvent, screen } from "@testing-library/react";
import type { QuotaBand, QuotaStatus, Tier } from "../api";
import { renderWith } from "../test/render";
import { QuotaPanel } from "./Quota";

function status(band: QuotaBand, tier: Tier = "middle"): QuotaStatus {
  const used = { open: [0.4, 0.1], quiet: [0.86, 0.21], exhausted: [1.02, 0.4] }[band];
  return {
    tier,
    band,
    windows: [
      { span: "five_hours", used: used[0], release_at: band === "open" ? null : "2030-01-01T10:30:00Z" },
      { span: "seven_days", used: used[1], release_at: null },
    ],
    binding: band === "open" ? null : "five_hours",
    spent: [
      { shape: "direct", points: 75 },
      { shape: "ask", points: 25 },
    ],
  };
}

test("open: two window bars, the usual sentence, no release time", () => {
  renderWith(<QuotaPanel quota={status("open")} onTier={() => undefined} />);
  expect(screen.getAllByRole("meter")).toHaveLength(2);
  expect(screen.getByText("40%")).toBeDefined();
  expect(screen.getByText("Everything as usual.")).toBeDefined();
  expect(screen.queryByText(/Frees up at/)).toBeNull();
  expect(screen.getByText("Conversations")).toBeDefined();
  expect(screen.getByText("75%")).toBeDefined();
});

test("quiet: names the binding window and when it frees up", () => {
  renderWith(<QuotaPanel quota={status("quiet")} onTier={() => undefined} />);
  expect(screen.getByText("They won't speak up on their own for now (near the 5 hours limit).")).toBeDefined();
  expect(screen.getByText(/Frees up at/)).toBeDefined();
});

test("exhausted: says no new calls and when they resume", () => {
  renderWith(<QuotaPanel quota={status("exhausted")} onTier={() => undefined} />);
  expect(screen.getByText("No new calls until the 5 hours window frees up.")).toBeDefined();
  expect(screen.getByText(/Frees up at/)).toBeDefined();
  expect(screen.getByText("102%")).toBeDefined();
});

test("ultra: no bars at all, one sentence", () => {
  const q: QuotaStatus = { ...status("open", "ultra"), windows: [] };
  renderWith(<QuotaPanel quota={q} onTier={() => undefined} />);
  expect(screen.queryAllByRole("meter")).toHaveLength(0);
  expect(screen.getByText("No windows. The daily limits on speaking up still apply.")).toBeDefined();
});

test("the slider is labelled, announces the tier name and picks a tier", () => {
  const picked: Tier[] = [];
  renderWith(<QuotaPanel quota={status("open")} onTier={(t) => picked.push(t)} />);
  const slider = screen.getByRole("slider", { name: "Intensity" });
  expect(slider.getAttribute("aria-valuetext")).toBe("Middle");
  // jsdom does not step range inputs on arrow keys; a native range does.
  fireEvent.change(slider, { target: { value: "5" } });
  expect(picked).toEqual(["ultra"]);
});
