import { dayKey, dayLabel, nameList } from "./format";

const t = (key: string, vars?: Record<string, string | number>) =>
  ({ "list.separator": ", ", "list.more": "{names} +{count}" })[key]!.replace(/\{(\w+)\}/g, (_, k) => String(vars?.[k]));

test("a name list spells out three names and counts the rest", () => {
  expect(nameList(["A", "B"], t)).toBe("A, B");
  expect(nameList(["A", "B", "C"], t)).toBe("A, B, C");
  expect(nameList(["A", "B", "C", "D", "E"], t)).toBe("A, B, C +2");
});

test("a date pill names the day, and the year only when it is not this one", () => {
  const en = (key: string) => (key === "locale" ? "en" : key);
  const now = new Date(2026, 9, 30, 12);
  expect(dayLabel(new Date(2026, 9, 10, 8).toISOString(), en, now)).toBe("October 10");
  expect(dayLabel(new Date(2025, 9, 10, 8).toISOString(), en, now)).toBe("October 10, 2025");
  expect(dayKey(new Date(2026, 9, 10, 0, 5).toISOString())).toBe(dayKey(new Date(2026, 9, 10, 23, 55).toISOString()));
  expect(dayKey(new Date(2026, 9, 10, 23, 55).toISOString())).not.toBe(dayKey(new Date(2026, 9, 11, 0, 5).toISOString()));
});
