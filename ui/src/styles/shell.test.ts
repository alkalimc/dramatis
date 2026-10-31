// The page never scrolls: the window is the app, and every pane scrolls inside itself.
// jsdom does no layout, so this reads the rules that make it so.
import { expect, test } from "vitest";
import base from "./base.css?raw";
import chat from "./chat.css?raw";
import layout from "./layout.css?raw";

/** The declarations of the first rule whose selector list is exactly `selector`. */
function rule(css: string, selector: string): string {
  const flat = css.replace(/\/\*[\s\S]*?\*\//g, "");
  for (const m of flat.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const list = m[1].split(",").map((s) => s.trim());
    if (list.join(",") === selector) return m[2];
  }
  throw new Error(`no rule for ${selector}`);
}

test("html, body and #root fill the window and never scroll", () => {
  const css = rule(base, "html,body,#root");
  expect(css).toMatch(/height:\s*100%/);
  expect(css).toMatch(/overflow:\s*hidden/);
  // `clip` also refuses scrolling by focus and scrollIntoView.
  expect(css).toMatch(/overflow:\s*clip/);
});

test("the frame is exactly the window and clips what the panes do not hold", () => {
  const app = rule(layout, ".app");
  expect(app).toMatch(/height:\s*100%/);
  expect(app).toMatch(/overflow:\s*hidden/);
  for (const sel of [".stage", ".body", ".main"]) {
    const css = rule(layout, sel);
    expect(css, sel).toMatch(/min-width:\s*0/);
    expect(css, sel).toMatch(/min-height:\s*0/);
  }
});

test("screens outside the frame scroll inside themselves", () => {
  expect(rule(base, "#root > :not(.app)")).toMatch(/overflow-y:\s*auto/);
});

test("the side panel scrolls in its own body, not as a whole", () => {
  expect(rule(layout, ".drawer")).toMatch(/overflow:\s*hidden/);
  expect(rule(layout, ".drawer-scroll")).toMatch(/overflow-y:\s*auto/);
});

test("a search field and its filters and button share one control height", () => {
  const css = rule(base, ".field-row > input,.field-row > select,.field-row > button");
  expect(css).toMatch(/height:\s*var\(--ctl\)/);
  expect(css).toMatch(/min-height:\s*var\(--ctl\)/);
  expect(rule(base, ".row.field-row")).toMatch(/align-items:\s*stretch/);
});

test("the forward picker's rows start at the left edge, not centred like a plain button", () => {
  expect(rule(chat, ".forward-row")).toMatch(/justify-content:\s*flex-start/);
});

test("the list a select opens has the theme's surface and ink, not the browser's white", () => {
  const css = rule(base, "select option");
  expect(css).toMatch(/background:\s*var\(--raised\)/);
  expect(css).toMatch(/color:\s*var\(--ink\)/);
});
