// The fixture backend's content sentinels (U-07): what scripts/check-bundle.mjs looks for in a
// production renderer besides its two markers. The fixture marker and `src/fixture/` paths
// (vite.config.ts, noFixtureModules) catch a fixture module by where it is and what it is labelled;
// the sentinels catch one by what it holds, so a copy moved out of `src/fixture/` without its
// marker is found as well.
//
// Derived from the fixture sources (src/fixture/**/*.ts, tests excluded) on every build, so a new
// fixture string is covered without a list to keep:
// - string literals and template parts of at least MIN_LENGTH printable ASCII characters (no
//   backslash, no `</`, which minifiers escape) that are not plain words: one made only of
//   letters, digits and `_$.-` is a sentinel at 20 characters or more, since a shorter one (a
//   field name, `unlocked`) also turns up as a property in any bundle;
// - integer literals of at least 7 digits that do not end in 0 (the PRNG seed 20261006): a round
//   number is minified to `5e9` and is not distinctive anyway;
// - minus SHARED, the fixture literals the rest of `src/` holds too (error codes the fixture
//   returns and the generated bindings list). SHARED is a reviewed list rather than computed on
//   each build: computed, a fixture copy outside `src/fixture/` would exempt its own literals.
//   src/fixture/sentinels.test.ts keeps it equal to the actual overlap;
// - minus GENERIC, fixture literals any dependency could hold (2^32, a PRNG constant, the base58
//   alphabet). Add one only when a production dependency collides with it, never to let fixture
//   code through; the test checks each is still a fixture literal.
//
// Sentinels come from the files in `src/fixture/`: a module moved out of it (rather than copied)
// takes its literals along. src/fixture/fixture.test.ts pins the list of fixture modules, so a
// move fails there.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, sep } from "node:path";
import { parseSync, transformWithOxc, Visitor } from "vite";

export const MIN_LENGTH = 8;
const MIN_WORD_LENGTH = 20;
const MIN_DIGITS = 7;

/** Fixture literals that non-fixture source in `src/` (tests excluded) also holds. */
export const SHARED = [
  "backup.already_exists",
  "compat.network_mismatch",
  "console.authorization_required",
  "contact.already_contact",
  "contact.details_too_long",
  "contact.payment_locked",
  "identity.keys_missing",
  "insufficient_credits",
  "platform.broadcast_unknown",
  "platform.feature_off",
  "platform.grant_exceeded",
  "platform.grant_invalid",
  "platform.insufficient_credits",
  "platform.lease_expired",
  "platform.lease_revoked",
  "platform.needs_grant",
  "platform.not_implemented",
  "platform.signer_unavailable",
  "psbt.fee_rate_too_high",
  "registration.funding_unknown_recent",
  "registration.in_progress",
  "send.amount_exceeds_balance",
  "send.amount_with_fee_exceeds_balance",
  "send.broadcast_rejected",
  "send.invalid_address",
  "vault.quick_unlock_limit_exceeded",
  "vault.quick_unlock_unavailable",
  "vault.wrong_passphrase",
];

/** Fixture literals too generic to be sentinels. */
export const GENERIC = [
  "#4294967296",
  "#1831565813",
  "#12345678",
  "#9999999",
  "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz",
  " characters",
  " failed)",
];

function* walk(dir) {
  for (const name of readdirSync(dir).sort()) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) yield* walk(path);
    else yield path;
  }
}

/** The renderer's TypeScript sources under `<app>/src`, tests excluded, as [path, isFixture]. */
export function* sources(app) {
  const src = join(app, "src");
  for (const path of walk(src)) {
    if (!/\.tsx?$/.test(path) || /\.test\.tsx?$/.test(path) || path.endsWith(".d.ts")) continue;
    yield [path, relative(src, path).startsWith(`fixture${sep}`)];
  }
}

/** The string and integer literals of a TypeScript file's emitted JavaScript: types and import
 * specifiers left out. Integers as `#<decimal>`. */
export async function literalsOf(path) {
  const ts = readFileSync(path, "utf8");
  const { code } = await transformWithOxc(ts, path, { lang: path.endsWith("x") ? "tsx" : "ts", jsx: "preserve" });
  const { program, errors } = parseSync(path.replace(/\.tsx?$/, ".jsx"), code);
  if (errors.length > 0) throw new Error(`${path}: ${errors[0].message}`);
  const specifiers = new Set();
  const specifier = (node) => node.source && specifiers.add(node.source.start);
  const out = new Set();
  new Visitor({
    ImportDeclaration: specifier,
    ImportExpression: specifier,
    ExportNamedDeclaration: specifier,
    ExportAllDeclaration: specifier,
    Literal(node) {
      if (specifiers.has(node.start)) return;
      if (typeof node.value === "string") out.add(node.value);
      else if (Number.isSafeInteger(node.value)) out.add(`#${node.value}`);
    },
    TemplateLiteral(node) {
      for (const quasi of node.quasis) if (quasi.value.cooked) out.add(quasi.value.cooked);
    },
  }).visit(program);
  return out;
}

function distinctive(literal) {
  if (literal.startsWith("#")) return literal.length > MIN_DIGITS && !literal.endsWith("0");
  if (literal.length < MIN_LENGTH || !/^[\x20-\x7e]*$/.test(literal) || literal.includes("\\") || literal.includes("</")) return false;
  return !/^[\w$.-]*$/.test(literal) || literal.length >= MIN_WORD_LENGTH;
}

/** The distinctive fixture literals and those the rest of `src/` also holds. */
export async function overlap(app) {
  const fixture = new Map();
  const rest = new Set();
  for (const [path, isFixture] of sources(app)) {
    for (const literal of await literalsOf(path)) {
      if (!isFixture) rest.add(literal);
      else if (distinctive(literal) && !fixture.has(literal)) fixture.set(literal, relative(app, path));
    }
  }
  const shared = [...fixture.keys()].filter((l) => rest.has(l)).sort();
  return { fixture, shared };
}

/** The sentinels, each with the fixture file it comes from. */
export async function fixtureSentinels(app) {
  const { fixture } = await overlap(app);
  for (const literal of [...SHARED, ...GENERIC]) fixture.delete(literal);
  return fixture;
}

/** Whether `text` (built JS, HTML or CSS) holds `sentinel` in any form a minifier writes it. */
export function holds(text, sentinel) {
  if (sentinel.startsWith("#")) {
    const n = Number(sentinel.slice(1));
    return [String(n), `0x${n.toString(16)}`].some((f) => new RegExp(`(?<![\\w$.])${f}(?![\\w$.])`, "i").test(text));
  }
  return [sentinel, sentinel.replaceAll('"', '\\"'), sentinel.replaceAll("'", "\\'")].some((f) => text.includes(f));
}
