// The page never scrolls: the window is the app, and every pane scrolls inside itself.
// jsdom does no layout, so this reads the rules that make it so.
import { expect, test } from "vitest";
import base from "./base.css?raw";
import chat from "./chat.css?raw";
import layout from "./layout.css?raw";
import panels from "./panels.css?raw";
import { CHAT_MIN_REM, DOCK_MIN_REM, LIST_FOLDS, PANEL_REM } from "../lib/breakpoints";
import tokens from "./tokens.css?raw";

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

test("every people row (contacts, forward, group pickers, members, @ mentions) starts at the left edge, not centred like a plain button", () => {
  expect(rule(layout, "button.person-row")).toMatch(/justify-content:\s*flex-start/);
  // The mention popup draws the same row; it only sets its height.
  expect(rule(chat, ".mention-pop .person-row")).toMatch(/height:/);
});

/** The declarations of the first rule for `selector` inside `@<kind> <head> { ... }`. */
function inQuery(css: string, head: string, selector: string): string {
  const flat = css.replace(/\/\*[\s\S]*?\*\//g, "");
  const start = flat.indexOf(`${head} {`);
  if (start < 0) throw new Error(`no query ${head}`);
  const block = flat.slice(start).split(/\n}\n/)[0];
  return rule(block.slice(block.indexOf("{") + 1), selector);
}

test("shrinking the window closes the right panel first, keeps the conversation, then folds the chat list", () => {
  // The panel docks only while the conversation keeps its least width beside it.
  expect(DOCK_MIN_REM).toBe(CHAT_MIN_REM + PANEL_REM);
  expect(tokens).toMatch(new RegExp(`--info-w:\\s*${PANEL_REM}rem`));
  expect(rule(layout, ".body")).toMatch(/container:\s*body\s*\/\s*inline-size/);
  expect(inQuery(layout, `@container body (max-width: ${DOCK_MIN_REM}rem)`, ".drawer")).toMatch(/position:\s*absolute/);
  expect(inQuery(layout, `@media (max-width: ${LIST_FOLDS}px)`, ".chat-list")).toMatch(/width:\s*4\.5rem/);
  // Order: the narrowest window that still docks the panel (rail, widest-folded list is not
  // yet in play, list at its least) is wider than the one where the list folds.
  const rail = 5.3 * 15;
  const listLeast = 16 * 15;
  expect(rail + listLeast + DOCK_MIN_REM * 15).toBeGreaterThan(LIST_FOLDS);
});

test("roster and world are pages that scroll inside themselves", () => {
  expect(rule(layout, ".page-scroll")).toMatch(/overflow-y:\s*auto/);
  expect(rule(layout, ".main.library,.main.page")).toMatch(/position:\s*relative/);
});

test("the list a select opens has the theme's surface and ink, not the browser's white", () => {
  const css = rule(base, "select option");
  expect(css).toMatch(/background:\s*var\(--raised\)/);
  expect(css).toMatch(/color:\s*var\(--ink\)/);
});

test("the settings card is one size for every page: a fixed height, with only the body scrolling", () => {
  const card = rule(layout, ".dialog.settings-card");
  expect(card).toMatch(/height:\s*min\(var\(--settings-h\),\s*90vh\)/);
  expect(card).toMatch(/overflow:\s*hidden/);
  expect(rule(layout, ".settings-scroll")).toMatch(/overflow-y:\s*auto/);
  // No page or part sets a size of its own inside the card.
  for (const css of [layout, panels]) {
    const flat = css.replace(/\/\*[\s\S]*?\*\//g, "");
    for (const m of flat.matchAll(/([^{}]*settings-(?:page|card|scroll)[^{}]*)\{([^{}]*)\}/g)) {
      if (m[1].includes(".dialog.settings-card") && !m[1].includes(",")) continue;
      expect(m[2], m[1]).not.toMatch(/(^|[\s;])(?:max-)?height\s*:|min-height\s*:\s*[1-9]/);
    }
  }
});
