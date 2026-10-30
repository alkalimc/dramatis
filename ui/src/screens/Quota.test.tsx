import { fireEvent, screen, within } from "@testing-library/react";
import type { QuotaBand, QuotaStatus, Tier } from "../api";
import { renderWith } from "../test/render";
import { QuotaPanel } from "./Quota";

function status(band: QuotaBand, tier: Tier = "middle"): QuotaStatus {
  const used = { open: 0.4, quiet: 0.86, exhausted: 1.02 }[band];
  return {
    tier,
    band,
    window: { used, release_at: band === "open" ? null : "2030-01-01T10:30:00Z" },
    spent: [
      { shape: "direct", calls: 75 },
      { shape: "host", calls: 25 },
    ],
    visits_per_day: 5,
    events_max: 4,
  };
}

test("open: one window bar, the usual sentence, no release time", () => {
  renderWith(<QuotaPanel quota={status("open")} onTier={() => undefined} />);
  expect(screen.getAllByRole("meter")).toHaveLength(1);
  expect(screen.getByText("40%")).toBeDefined();
  expect(screen.getByText("Everything as usual.")).toBeDefined();
  expect(screen.queryByText(/Frees up at/)).toBeNull();
  expect(screen.getByText("Conversations")).toBeDefined();
  expect(screen.getByText("75%")).toBeDefined();
});

test("quiet: says why and when it frees up", () => {
  renderWith(<QuotaPanel quota={status("quiet")} onTier={() => undefined} />);
  expect(screen.getByText("They won't speak up on their own for now (near the hourly limit).")).toBeDefined();
  expect(screen.getByText(/Frees up at/)).toBeDefined();
});

test("exhausted: says no new calls and when they resume", () => {
  renderWith(<QuotaPanel quota={status("exhausted")} onTier={() => undefined} />);
  expect(screen.getByText("No new calls until the hourly window frees up.")).toBeDefined();
  expect(screen.getByText(/Frees up at/)).toBeDefined();
  expect(screen.getByText("102%")).toBeDefined();
});

test("ultra: no bars at all, one sentence", () => {
  const q: QuotaStatus = { ...status("open", "ultra"), window: null };
  renderWith(<QuotaPanel quota={q} onTier={() => undefined} />);
  expect(screen.queryAllByRole("meter")).toHaveLength(0);
  expect(screen.getByText("No hourly limit. The daily limits on speaking up still apply.")).toBeDefined();
});

test("the intensity is a labelled choice that shows the tier and picks another", () => {
  const picked: Tier[] = [];
  renderWith(<QuotaPanel quota={status("open")} onTier={(t) => picked.push(t)} />);
  const group = screen.getByRole("group", { name: "Intensity" });
  expect((within(group).getByRole("radio", { name: "Middle" }) as HTMLInputElement).checked).toBe(true);
  fireEvent.click(within(group).getByRole("radio", { name: "Ultra" }));
  expect(picked).toEqual(["ultra"]);
});

test("says what the intensity also sets", () => {
  renderWith(<QuotaPanel quota={status("open")} onTier={() => undefined} />);
  expect(screen.getByText(/up to 5 visits and 4 happenings on board a day/)).toBeDefined();
});
