import { pieces, snippet, termsOf } from "./snippet";

test("terms are lower-case, unique, longest first", () => {
  expect(termsOf("Flood  flood ward")).toEqual(["flood", "ward"]);
  expect(termsOf("a abc")).toEqual(["abc", "a"]);
});

test("a short text stays whole; a long one is cut around the first hit", () => {
  expect(snippet("a short line", ["short"])).toBe("a short line");
  const long = `${"x ".repeat(100)}needle${" y".repeat(100)}`;
  const s = snippet(long, ["needle"], 20);
  expect(s.startsWith("…")).toBe(true);
  expect(s.endsWith("…")).toBe(true);
  expect(s).toContain("needle");
  expect(s.length).toBeLessThan(50);
});

test("hits are marked whatever their case, and regex characters are plain", () => {
  expect(pieces("The Flood came. flood!", ["flood"])).toEqual([
    { text: "The ", hit: false },
    { text: "Flood", hit: true },
    { text: " came. ", hit: false },
    { text: "flood", hit: true },
    { text: "!", hit: false },
  ]);
  expect(pieces("a (b) c", ["(b)"]).map((p) => p.hit)).toEqual([false, true, false]);
  expect(pieces("nothing", [])).toEqual([{ text: "nothing", hit: false }]);
});
