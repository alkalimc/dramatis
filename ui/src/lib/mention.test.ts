import { filterMembers, highlightMentions, insertMention, mentionAt } from "./mention";

test("an @ at the start of a word opens a mention; a space or another word ends it", () => {
  expect(mentionAt("hello @mi", 9)).toEqual({ start: 6, query: "mi" });
  expect(mentionAt("@", 1)).toEqual({ start: 0, query: "" });
  expect(mentionAt("line one\n@or", 12)).toEqual({ start: 9, query: "or" });
  // Inside a word it is an address, not a mention; a space after the name closes it.
  expect(mentionAt("mail me@mi", 10)).toBeNull();
  expect(mentionAt("hello @mira and", 15)).toBeNull();
  // Only what is before the caret counts.
  expect(mentionAt("hi @mira", 4)).toEqual({ start: 3, query: "" });
});

test("choosing a member replaces what was typed with @name and a space, and moves the caret after it", () => {
  const at = mentionAt("hi @mi there", 6)!;
  // A space follows already: it is used, not doubled.
  expect(insertMention("hi @mi there", at, 6, "Mira Vale")).toEqual({ text: "hi @Mira Vale there", caret: 14 });
  expect(insertMention("@mi", mentionAt("@mi", 3)!, 3, "Mira Vale")).toEqual({ text: "@Mira Vale ", caret: 11 });
});

test("members that start with the typed text come first, then those that contain it", () => {
  const all = [{ name: "Tess Arden" }, { name: "Oren Lark" }, { name: "Mira Vale" }, { name: "Kael Moor" }];
  expect(filterMembers(all, "").map((m) => m.name)).toEqual(["Tess Arden", "Oren Lark", "Mira Vale", "Kael Moor"]);
  expect(filterMembers(all, "or").map((m) => m.name)).toEqual(["Oren Lark", "Kael Moor"]);
  expect(filterMembers(all, "ar").map((m) => m.name)).toEqual(["Tess Arden", "Oren Lark"]);
  expect(filterMembers(all, "mi").map((m) => m.name)).toEqual(["Mira Vale"]);
  expect(filterMembers(all, "zzz")).toEqual([]);
});

test("@name of a member is set apart in rendered text, never inside a tag", () => {
  const html = '<p>@Mira Vale and @Oren, see <a href="mailto:x@Mira">x</a> and @Nobody</p>';
  expect(highlightMentions(html, ["Mira Vale", "Oren"])).toBe(
    '<p><span class="mention">@Mira Vale</span> and <span class="mention">@Oren</span>, see <a href="mailto:x@Mira">x</a> and @Nobody</p>',
  );
  expect(highlightMentions("<p>no mention</p>", ["Mira"])).toBe("<p>no mention</p>");
  // A name with markup characters is matched as the renderer wrote it.
  expect(highlightMentions("<p>@Ann &amp; Bo</p>", ["Ann & Bo"])).toBe('<p><span class="mention">@Ann &amp; Bo</span></p>');
});
