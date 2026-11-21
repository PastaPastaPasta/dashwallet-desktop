// Keeps converted screens converted (MP-14): a `.tsx` file listed in CONVERTED (a file, or every
// file under a directory) may not show text that bypasses the catalog: JSX text, or a literal in
// an attribute that is read or shown (`title`, `aria-label`, `placeholder`, …). Each later slice
// adds the paths it converts.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { parseSync, Visitor } from "vite";
import { describe, expect, it } from "vitest";

/** Paths under src/ whose `.tsx` files take their text from the catalog, by screen area. Each area's
 * pull request fills its own list. */
const AREAS: Record<string, string[]> = {
  shell: ["components/shell.tsx", "components/networkPicker.tsx"],

  lock: ["screens/LockScreen.tsx"],

  settings: ["screens/settings/General.tsx", "screens/settings/LocalCurrency.tsx", "screens/settings/Privacy.tsx", "screens/settings/SettingsPage.tsx", "screens/settings/Updates.tsx", "screens/settings/parts.tsx"],

  components: [],

  app: [],

  onboarding: [],

  home: [],

  activity: [],

  receive: [],

  pay: [],

  contacts: [],

  dashpay: [],

  coinjoin: [],

  notifications: [],

  masternodes: [],

  tools: [],

  transfer: [],
};

export const CONVERTED = Object.values(AREAS).flat();

/** Text that may stay literal: `file: [text, …]`. Keep it short and say why. */
const ALLOWED: Record<string, string[]> = {};

/** Attributes whose value a user reads or hears. Props of our components that carry copy too. */
const TEXT_ATTRIBUTES = new Set(["title", "aria-label", "aria-description", "aria-placeholder", "aria-roledescription", "placeholder", "alt", "label", "text", "why", "confirm", "message", "submitLabel"]);

/** Functions whose arguments a user reads: a toast's text. */
const TEXT_CALLS = new Set(["showToast"]);

const src = resolve(import.meta.dirname, "..");
const LETTER = /\p{L}/u;

function files(path: string): string[] {
  const full = join(src, path);
  if (!statSync(full).isDirectory()) return [full];
  return readdirSync(full, { recursive: true, encoding: "utf8" })
    .filter((f) => f.endsWith(".tsx"))
    .map((f) => join(full, f));
}

interface Node {
  type: string;
  start: number;
  [k: string]: unknown;
}

/** The hard-coded strings in a `.tsx` source, as `line: text`. */
export function hardCoded(file: string, source: string): string[] {
  const { program, errors } = parseSync(file, source, { lang: "tsx" });
  if (errors.length) throw new Error(`${file}: ${errors[0].message}`);
  const found: { at: number; text: string }[] = [];
  const add = (at: number, text: string) => {
    const t = text.replace(/\s+/g, " ").trim();
    if (LETTER.test(t)) found.push({ at: at + text.search(/\S/), text: t });
  };
  /** String literals an expression can evaluate to (`c ? "a" : "b"`, `x ?? "a"`, `` `a ${b}` ``). */
  const literals = (e: Node | null | undefined) => {
    if (!e) return;
    if (e.type === "Literal" && typeof e.value === "string") add(e.start, e.value);
    else if (e.type === "TemplateLiteral") for (const q of e.quasis as { start: number; value: { cooked: string } }[]) add(q.start, q.value.cooked);
    else if (e.type === "ConditionalExpression") {
      literals(e.consequent as Node);
      literals(e.alternate as Node);
    } else if (e.type === "LogicalExpression" || (e.type === "BinaryExpression" && e.operator === "+")) {
      literals(e.left as Node);
      literals(e.right as Node);
    }
  };
  const children = (nodes: Node[]) => {
    for (const c of nodes) if (c.type === "JSXExpressionContainer") literals(c.expression as Node);
  };
  new Visitor({
    CallExpression: (n) => {
      const callee = n.callee as unknown as Node;
      if (callee.type === "Identifier" && TEXT_CALLS.has(callee.name as string)) for (const a of n.arguments as unknown as Node[]) literals(a);
    },
    JSXText: (n) => add(n.start, n.value),
    JSXElement: (n) => children(n.children as unknown as Node[]),
    JSXFragment: (n) => children(n.children as unknown as Node[]),
    JSXAttribute: (n) => {
      const name = n.name.type === "JSXIdentifier" ? n.name.name : `${n.name.namespace.name}:${n.name.name.name}`;
      if (!TEXT_ATTRIBUTES.has(name) || !n.value) return;
      const v = n.value as unknown as Node;
      if (v.type === "Literal") literals(v);
      else if (v.type === "JSXExpressionContainer") literals(v.expression as Node);
    },
  }).visit(program);
  const line = (at: number) => source.slice(0, at).split("\n").length;
  return found.sort((a, b) => a.at - b.at).map((f) => `${line(f.at)}: ${f.text}`);
}

describe("converted screens", () => {
  for (const path of CONVERTED)
    for (const file of files(path)) {
      const name = relative(src, file);
      it(`${name} has no hard-coded user-visible text`, () => {
        const allowed = new Set(ALLOWED[name] ?? []);
        const found = hardCoded(file, readFileSync(file, "utf8")).filter((f) => !allowed.has(f.slice(f.indexOf(": ") + 2)));
        expect(found).toEqual([]);
      });
    }

  it("catches JSX text, text attributes and literal children, and leaves code alone", () => {
    const found = hardCoded(
      "probe.tsx",
      [
        "export const A = ({ on }: { on: boolean }) => (",
        '  <div title="Hello" aria-label={on ? "Shown" : "Hidden"} className="no-copy" data-testid="x-y">',
        "    Some text {on && \"Yes\"} {`Count ${1}`} {t('ok')} {'…'}",
        '    <input placeholder="Type here" />',
        '    <Card why={"Because"} id="raw-id" />',
        '    <p title={"Copy " + name}>{name ?? "Nobody"}{"Left" || name}</p>',
        "  </div>",
        ");",
        'showToast("Copied"); other("not-copy");',
      ].join("\n"),
    );
    expect(found).toEqual(["2: Hello", "2: Shown", "2: Hidden", "3: Some text", "3: Yes", "3: Count", "4: Type here", "5: Because", "6: Copy", "6: Nobody", "6: Left", "9: Copied"]);
  });
});
