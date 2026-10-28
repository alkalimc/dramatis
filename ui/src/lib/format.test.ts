import { nameList } from "./format";

const t = (key: string, vars?: Record<string, string | number>) =>
  ({ "list.separator": ", ", "list.more": "{names} +{count}" })[key]!.replace(/\{(\w+)\}/g, (_, k) => String(vars?.[k]));

test("a name list spells out three names and counts the rest", () => {
  expect(nameList(["A", "B"], t)).toBe("A, B");
  expect(nameList(["A", "B", "C"], t)).toBe("A, B, C");
  expect(nameList(["A", "B", "C", "D", "E"], t)).toBe("A, B, C +2");
});
