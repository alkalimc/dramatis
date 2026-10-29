import { parseAst } from "vite";

// Every sentence the screens show goes through `t(...)`, so the pack can word it and the
// interface language can switch. This reads the sources and fails on the places that
// would skip it: words typed into JSX, words in the props a screen reader or a tooltip
// shows, dialog texts, and dates or numbers formatted by the browser's own locale.

const sources = import.meta.glob<string>("../**/*.{ts,tsx}", { query: "?raw", import: "default", eager: true });
/** Not user-facing: the wording table itself, fixtures, the dev entry and generated code. */
const SKIP = [/^i18n\//, /^api\/mock\//, /^test\//, /^dev\.ts$/, /^api\.gen\.ts$/, /\.test\.tsx?$/];
/** Props whose text a person reads or hears. */
const TEXT_PROPS = new Set(["aria-label", "aria-description", "aria-placeholder", "aria-roledescription", "aria-valuetext", "title", "placeholder", "alt", "label"]);
const DIALOGS = new Set(["alert", "confirm", "prompt"]);
const LOCALE_CALLS = /^toLocale(Date|Time)?String$/;
const FORMATTERS = new Set(["DateTimeFormat", "NumberFormat", "RelativeTimeFormat", "ListFormat", "PluralRules"]);
/** Literal text that may stand as it is: example values, not prose. */
const ALLOW = new Set<string>(["~/corpus/archive.folio", "https://api.example.com/v1"]);

const hasWords = (s: string) => /\p{L}/u.test(s) && !ALLOW.has(s.trim());

type Node = { type: string; [k: string]: unknown };
const isNode = (v: unknown): v is Node => typeof v === "object" && v !== null && typeof (v as Node).type === "string";

function walk(node: Node, visit: (n: Node) => void) {
  visit(node);
  for (const value of Object.values(node)) {
    if (Array.isArray(value)) value.filter(isNode).forEach((n) => walk(n, visit));
    else if (isNode(value)) walk(value, visit);
  }
}

/** The text of a string literal or a template (its fixed parts), else null. */
function text(n: unknown, plainOnly = false): string | null {
  if (!isNode(n)) return null;
  if (n.type === "Literal" && typeof n.value === "string") return n.value;
  if (n.type === "TemplateLiteral" && !(plainOnly && (n.expressions as unknown[]).length > 0)) {
    const quasis = n.quasis as { value: { cooked: string } }[];
    return quasis.map((q) => q.value.cooked).join("");
  }
  return null;
}

function calleeName(call: Node): string {
  const c = call.callee as Node;
  if (c.type === "Identifier") return c.name as string;
  if (c.type === "MemberExpression") return ((c.property as Node).name as string) ?? "";
  return "";
}

export function problems(source: string, lang: "ts" | "tsx"): string[] {
  const found: string[] = [];
  const ast = parseAst(source, { lang }) as unknown as Node;
  const at = (n: Node) => source.slice(0, n.start as number).split("\n").length;
  walk(ast, (n) => {
    if (n.type === "JSXElement" || n.type === "JSXFragment") {
      for (const child of n.children as Node[]) {
        if (child.type === "JSXText" && hasWords(child.value as string)) found.push(`${at(child)}: JSX text "${(child.value as string).trim()}"`);
        if (child.type === "JSXExpressionContainer" && hasWords(text(child.expression, true) ?? "")) found.push(`${at(child)}: JSX text "${text(child.expression, true)}"`);
      }
    }
    if (n.type === "JSXAttribute") {
      const name = (n.name as Node).name as string;
      const value = isNode(n.value) && n.value.type === "JSXExpressionContainer" ? n.value.expression : n.value;
      if (TEXT_PROPS.has(name) && hasWords(text(value) ?? "")) found.push(`${at(n)}: ${name}="${text(value)}"`);
    }
    if (n.type === "CallExpression") {
      const name = calleeName(n);
      const args = n.arguments as Node[];
      if (DIALOGS.has(name) && hasWords(text(args[0]) ?? "")) found.push(`${at(n)}: ${name}("${text(args[0])}")`);
      const first = args[0];
      const bare = !first || (first.type === "Identifier" && first.name === "undefined");
      if (LOCALE_CALLS.test(name) && bare) found.push(`${at(n)}: ${name}() formats with the browser's locale`);
    }
    if (n.type === "NewExpression") {
      const c = n.callee as Node;
      const member = c.type === "MemberExpression" && (c.object as Node).name === "Intl" ? ((c.property as Node).name as string) : "";
      const first = (n.arguments as Node[])[0];
      const bare = !first || (first.type === "Identifier" && first.name === "undefined");
      if (FORMATTERS.has(member) && bare) found.push(`${at(n)}: Intl.${member} formats with the browser's locale`);
    }
  });
  return found;
}

test("the checker catches the usual ways of skipping the wording", () => {
  const bad = `
    const a = <button title="Close" aria-label={"Go"}>Save <b>{"now"}</b></button>;
    const b = <input placeholder={\`Name\`} />;
    window.confirm("Sure?");
    new Date().toLocaleTimeString(undefined, {});
    new Intl.NumberFormat().format(1);
  `;
  expect(problems(bad, "tsx")).toHaveLength(8);
  const good = `
    const a = <button title={t("x")} aria-label={t("y")}>{t("z")} ×</button>;
    new Date().toLocaleTimeString(locale(), {});
    window.confirm(t("sure"));
  `;
  expect(problems(good, "tsx")).toEqual([]);
});

test("no screen or component carries wording of its own", () => {
  const all = Object.entries(sources)
    .map(([path, source]) => ({ file: path.replace(/^\.\.\//, ""), source }))
    .filter((f) => !SKIP.some((re) => re.test(f.file)));
  expect(all.length).toBeGreaterThan(30);
  const found = all.flatMap(({ file, source }) => problems(source, file.endsWith("x") ? "tsx" : "ts").map((p) => `${file}:${p}`));
  expect(found).toEqual([]);
});
