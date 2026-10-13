// The fixture backend's content sentinels (U-07): what scripts/check-bundle.mjs looks for in a
// production renderer besides its two markers. The fixture marker and `src/fixture/` paths
// (vite.config.ts, noFixtureModules) catch a fixture module by where it is and what it is labelled;
// the sentinels catch one by what it holds, so a copy moved out of `src/fixture/` without its
// marker is found as well.
//
// Derived from the fixture modules (every source file in `src/fixture/` of an EXTENSIONS type,
// the `*.test.ts` vitest runs excluded) on every build, so a new fixture string is covered without a list to keep:
// - string literals and template parts of at least MIN_LENGTH printable ASCII characters (no
//   backslash, no `</`, which minifiers escape) that are not plain words: one that is only
//   letters, digits and `_$.-` once surrounding spaces and punctuation are trimmed is a sentinel
//   at 20 characters or more, since a shorter one (a field name, `unlocked`, ` failed)`) also
//   turns up in any bundle;
// - integer literals of at least 7 digits that do not end in 0 (the PRNG seed 20261006): a round
//   number is minified to `5e9` and is not distinctive anyway;
// - minus the shared literals: those the generated bindings also hold (GENERATED, the error codes
//   the fixture returns). Computed from those files, not kept by hand: they are exactly what
//   dw-app's codegen writes (`codegen::tests::bindings_are_current` refuses an extra or edited
//   file), so a fixture copy anywhere else exempts nothing;
// - minus GENERIC, fixture literals a production dependency holds too. Empty: an entry needs its
//   reason, a generic shape (a power of two, a published PRNG constant, an alphabet;
//   src/fixture/sentinels.test.ts) and a real collision, since check-bundle fails a production
//   build that does not hold it.
//
// Every fixture module must contribute a sentinel (or be in NO_SENTINEL with a reason), and a file
// in `src/fixture/` of any other type fails, so no fixture module goes unread. A module moved out
// of `src/fixture/` (rather than copied) takes its literals along: src/fixture/fixture.test.ts pins
// the list of fixture files, so a move fails there. The check works per literal, not per function:
// a copy whose bundled part holds none of them (an import of one function, the rest tree-shaken)
// is not found.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { extname, join, relative, sep } from "node:path";
import { parseSync, transformWithOxc, Visitor } from "vite";

export const MIN_LENGTH = 8;
const MIN_WORD_LENGTH = 20;
const MIN_DIGITS = 7;

/** Every extension Vite loads as a JavaScript module, with the oxc language it is parsed as. */
export const EXTENSIONS = {
  ".ts": "ts",
  ".mts": "ts",
  ".cts": "ts",
  ".tsx": "tsx",
  ".js": "js",
  ".mjs": "js",
  ".cjs": "js",
  ".jsx": "jsx",
};

/** The generated files whose literals are shared with the fixture rather than its own. */
export const GENERATED = "src/api/generated";

/** Files in `src/fixture/` that are neither modules nor tests: test data no module imports. */
export const TEST_DATA = { "vectors/accept.json": "accept_grant_vectors for accept.test.ts", "vectors/history.json": "history_page vectors for history.test.ts" };

/** Fixture modules with no sentinel of their own, and why that is safe. */
export const NO_SENTINEL = {};

/** Fixture literals a production dependency holds too, each with its reason: `{ literal, reason }`. */
export const GENERIC = [];

/** Files no tool reads: Finder's folder data and editor swap and backup files. */
export const IGNORED = /(^|\/)\.DS_Store$|\.sw[op]$|~$/;

function* walk(dir) {
  for (const name of readdirSync(dir).sort()) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) yield* walk(path);
    else yield path;
  }
}

// Exactly what vitest runs (vite.config.ts `test.include`) and TypeScript's declarations.
const isTest = (path) => /\.test\.ts$/.test(path) || /\.d\.ts$/.test(path);

/** The fixture modules: every source file under `<app>/src/fixture`, tests excluded. A file of any
 * other type fails unless it is TEST_DATA. */
export function* fixtureModules(app) {
  const dir = join(app, "src", "fixture");
  for (const path of walk(dir)) {
    const name = relative(dir, path).split(sep).join("/");
    if (isTest(path) || name in TEST_DATA || IGNORED.test(name)) continue;
    if (!(extname(path) in EXTENSIONS)) throw new Error(`src/fixture/${name}: not a module type the sentinels read (${Object.keys(EXTENSIONS).join(" ")})`);
    yield path;
  }
}

/** The string and integer literals of a module's emitted JavaScript: types and import specifiers
 * left out. Integers as `#<decimal>`. */
export async function literalsOf(path) {
  const lang = EXTENSIONS[extname(path)];
  if (!lang) throw new Error(`${path}: not a module type the sentinels read`);
  const source = readFileSync(path, "utf8");
  const { code } = await transformWithOxc(source, path, { lang, jsx: "preserve" });
  const { program, errors } = parseSync(path.slice(0, -extname(path).length) + ".jsx", code);
  if (errors.length > 0) throw new Error(`${path}: ${errors[0].message}`);
  const specifiers = new Set();
  const specifier = (node) => node.source && specifiers.add(node.source.start);
  const out = new Set();
  new Visitor({
    ImportDeclaration: specifier,
    ImportExpression: specifier,
    ExportNamedDeclaration: specifier,
    ExportAllDeclaration: specifier,
    CallExpression(node) {
      if (node.callee.type === "Identifier" && node.callee.name === "require" && node.arguments[0]) specifiers.add(node.arguments[0].start);
    },
    Literal(node) {
      if (specifiers.has(node.start)) return;
      if (typeof node.value === "string") out.add(node.value);
      else if (Number.isSafeInteger(node.value)) out.add(`#${node.value}`);
    },
    TemplateLiteral(node) {
      if (specifiers.has(node.start)) return;
      for (const quasi of node.quasis) if (quasi.value.cooked) out.add(quasi.value.cooked);
    },
  }).visit(program);
  return out;
}

export function distinctive(literal) {
  if (literal.startsWith("#")) return literal.length > MIN_DIGITS && !literal.endsWith("0");
  if (literal.length < MIN_LENGTH || !/^[\x20-\x7e]*$/.test(literal) || literal.includes("\\") || literal.includes("</")) return false;
  const word = literal.replace(/^[\s\p{P}]+|[\s\p{P}]+$/gu, "");
  return !/^[\w$.-]*$/.test(word) || word.length >= MIN_WORD_LENGTH;
}

/** The literals of the generated bindings (GENERATED). */
export async function generatedLiterals(app) {
  const out = new Set();
  for (const path of walk(join(app, GENERATED))) if (extname(path) in EXTENSIONS) for (const literal of await literalsOf(path)) out.add(literal);
  return out;
}

/** The sentinels, each with the fixture module it first comes from (relative to `app`), and each
 * fixture module's sentinels. */
export async function fixtureSentinels(app) {
  const generated = await generatedLiterals(app);
  const generic = new Set(GENERIC.map((g) => g.literal));
  const sentinels = new Map();
  const byModule = new Map();
  for (const path of fixtureModules(app)) {
    const module = relative(app, path).split(sep).join("/");
    const own = [...(await literalsOf(path))].filter((l) => distinctive(l) && !generated.has(l) && !generic.has(l));
    byModule.set(module, own);
    for (const literal of own) if (!sentinels.has(literal)) sentinels.set(literal, module);
    const exempt = module.slice("src/fixture/".length) in NO_SENTINEL;
    if (own.length === 0 && !exempt) throw new Error(`${module}: a fixture module with no sentinel: give it a distinctive literal or list it in NO_SENTINEL with a reason`);
  }
  return { sentinels, byModule };
}

/** Whether `text` (built JS, HTML or CSS) holds `sentinel` in any form a minifier writes it. */
export function holds(text, sentinel) {
  if (sentinel.startsWith("#")) {
    const n = Number(sentinel.slice(1));
    return [String(n), `0x${n.toString(16)}`].some((f) => new RegExp(`(?<![\\w$.])${f}(?![\\w$.])`, "i").test(text));
  }
  return [sentinel, sentinel.replaceAll('"', '\\"'), sentinel.replaceAll("'", "\\'")].some((f) => text.includes(f));
}
