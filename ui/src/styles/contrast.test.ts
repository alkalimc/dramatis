// WCAG contrast of the token pairs in tokens.css, for the dark and the light palette.
import { describe, expect, it } from "vitest";
import css from "./tokens.css?raw";

function block(selector: string): Record<string, string> {
  const start = css.indexOf(`${selector} {`);
  const body = css.slice(start, css.indexOf("\n}", start));
  const out: Record<string, string> = {};
  for (const m of body.matchAll(/(--[\w-]+):\s*(#[0-9a-fA-F]{6})\b/g)) out[m[1]] = m[2];
  return out;
}

const dark = block(":root");
const light = { ...dark, ...block(':root[data-theme="light"]') };

function luminance(hex: string) {
  const [r, g, b] = [1, 3, 5].map((i) => {
    const c = parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}
function ratio(a: string, b: string) {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

const surfaces = ["--bg", "--panel", "--raised", "--field"];
const text = ["--ink", "--stamp", "--muted", "--accent", "--danger", "--warn"];
const hues = Array.from({ length: 8 }, (_, i) => `--hue-${i}`);

// [foreground, background, minimum ratio]
const pairs: [string, string, number][] = [
  ...surfaces.flatMap((s) => text.map((t) => [t, s, 4.5] as [string, string, number])),
  ...["--bg", "--panel", "--raised"].flatMap((s) => hues.map((h) => [h, s, 4.5] as [string, string, number])),
  ["--ink", "--own", 4.5],
  ["--stamp-own", "--own", 4.5],
  ["--accent", "--own", 4.5],
  ["--ink", "--rail", 4.5],
  ["--muted", "--rail", 4.5],
  ["--accent", "--rail", 4.5],
  ["--accent-ink", "--accent-fill", 4.5],
  ["--accent-ink", "--accent-fill-hover", 4.5],
  ["--accent-ink", "--accent-fill-press", 4.5],
  ["--accent-ink", "--danger-fill", 4.5],
  ["--accent-ink", "--camera", 3],
  ["--focus", "--panel", 3],
  ["--focus", "--bg", 3],
  ["--switch-off", "--panel", 3],
];

describe.each([
  ["dark", dark],
  ["light", light],
])("%s tokens", (_, tokens) => {
  it.each(pairs)("%s on %s is at least %s:1", (fg, bg, min) => {
    expect(ratio(tokens[fg], tokens[bg])).toBeGreaterThanOrEqual(min);
  });
});
