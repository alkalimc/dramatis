import { splitScene, speechOnly } from "./scene";

const star = { open: "*", close: "*" };

test("whole lines in the marker are scene lines; the rest is speech", () => {
  expect(splitScene("*She smiles.*\nI'll check that.", star)).toEqual([
    { kind: "scene", text: "She smiles." },
    { kind: "speech", text: "I'll check that." },
  ]);
});

test("speech lines stay together between scene lines", () => {
  const text = "First line.\nSecond line.\n*He leaves.*\nThird.";
  expect(splitScene(text, star)).toEqual([
    { kind: "speech", text: "First line.\nSecond line." },
    { kind: "scene", text: "He leaves." },
    { kind: "speech", text: "Third." },
  ]);
});

test("a marker inside a sentence, or bold, is not a scene line", () => {
  expect(splitScene("It was *very* late.", star)).toEqual([{ kind: "speech", text: "It was *very* late." }]);
  expect(splitScene("**Important.**", star)).toEqual([{ kind: "speech", text: "**Important.**" }]);
  expect(splitScene("**", star)).toEqual([{ kind: "speech", text: "**" }]);
});

test("surrounding whitespace is tolerated and blank speech dropped", () => {
  expect(splitScene("  *Rain.*  \n\n", star)).toEqual([{ kind: "scene", text: "Rain." }]);
  expect(splitScene("", star)).toEqual([]);
});

test("custom ASCII markers", () => {
  const paren = { open: "(", close: ")" };
  expect(splitScene("(She nods.)\nYes.\nI (think) so.", paren)).toEqual([
    { kind: "scene", text: "She nods." },
    { kind: "speech", text: "Yes.\nI (think) so." },
  ]);
});

test("custom full-width markers", () => {
  // Full-width parentheses, written as escapes so the source stays ASCII.
  const wide = { open: "\uFF08", close: "\uFF09" };
  const text = "\uFF08She laughs.\uFF09\nI will confirm this.";
  expect(splitScene(text, wide)).toEqual([
    { kind: "scene", text: "She laughs." },
    { kind: "speech", text: "I will confirm this." },
  ]);
  // With the default marker the same text is all speech.
  expect(splitScene(text, star)).toHaveLength(1);
});

test("multi-character markers", () => {
  const m = { open: "[[", close: "]]" };
  expect(splitScene("[[A door closes.]]\nWho's there?", m)[0]).toEqual({ kind: "scene", text: "A door closes." });
});

test("read-aloud text leaves the scene lines out", () => {
  expect(speechOnly("*She smiles.*\nHello.\n*Waves.*\nBye.", star)).toBe("Hello.\nBye.");
});
