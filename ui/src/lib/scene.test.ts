import { blocks, bubbles, splitScene, speechOnly, withoutCites } from "./scene";

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

test("citation marks and the space before them are not shown", () => {
  expect(withoutCites("I remember the street.[#dialogue:cf4c]\nNext [#voice:1a] line")).toBe(
    "I remember the street.\nNext line",
  );
});

test("several ids in one bracket go together", () => {
  expect(withoutCites("One.[#a:1,#b:2] Two [#p:3, #q:4] three [#r:5, s:6]")).toBe(
    "One. Two three",
  );
  expect(withoutCites("keep [#a:1, ] and [#a:1,]")).toBe("keep [#a:1, ] and [#a:1,]");
});

test("brackets that are not citation marks stay", () => {
  expect(withoutCites("[# a b] and [x]")).toBe("[# a b] and [x]");
});

// The same cases as the engine's citation tests (engine/crates/api/src/cites.rs).
test("the engine's citation cases", () => {
  expect(withoutCites("It works [#u2].")).toBe("It works.");
  expect(withoutCites("a [#u1, #u2] b [#x,y]")).toBe("a b");
  expect(withoutCites("two  \t[#a]\nnext [#b]  ")).toBe("two\nnext");
  expect(withoutCites("Yes. [#voice:ab12] Go.")).toBe("Yes. Go.");
  expect(withoutCites("Yes [#a:1,#b:2] and [#c:3, #d:4] go [#e:5, ].")).toBe("Yes and go [#e:5, ].");
  expect(withoutCites("see [1] and [#] and [#abc")).toBe("see [1] and [#] and [#abc");
  const long = `x [#${"a".repeat(129)}]`;
  expect(withoutCites(long)).toBe(long);
  expect(withoutCites("[#a b]")).toBe("[#a b]");
  expect(withoutCites("[# not an id] ok")).toBe("[# not an id] ok");
});

test("blocks: paragraphs apart, one label per run, scene lines between, marks-only line dropped", () => {
  const text = "One.\n\nTwo.\n*Rain.*\nThree.\n\n[#a:1]";
  expect(blocks(text, star)).toEqual([
    { kind: "speech", text: "One.", first: true },
    { kind: "speech", text: "Two.", first: false },
    { kind: "scene", text: "Rain." },
    { kind: "speech", text: "Three.", first: true },
  ]);
  expect(blocks("[#a:1]", star)).toEqual([]);
});

test("scene lines fold into the bubble they lead into; trailing ones close the last", () => {
  expect(bubbles("*Rain.*\nHello.\n\nSecond.\n*She leaves.*", star)).toEqual([
    { before: ["Rain."], text: "Hello.", after: [] },
    { before: [], text: "Second.", after: ["She leaves."] },
  ]);
  expect(bubbles("*Only a scene.*", star)).toEqual([{ before: ["Only a scene."], text: null, after: [] }]);
  expect(bubbles("", star)).toEqual([]);
});
