#!/usr/bin/env node
// Proves a built renderer does (or does not) hold the fixture backend's markers and content (U-07).
//
//   node scripts/check-bundle.mjs dist --production    none of them: `pnpm build` runs this,
//                                                      so every `tauri build` does too
//   node scripts/check-bundle.mjs dist-fixture --fixture   the positive control: every one is
//                                                      found in a fixture build
//
// The markers: the fixture's sentinel string (src/fixture/index.ts `FIXTURE_SENTINEL`) and a
// string only Tauri's mock IPC module (`@tauri-apps/api/mocks`) contains. The content: the
// distinctive literals of the fixture sources, derived when this runs (scripts/fixture-sentinels.mjs),
// so fixture code moved out of `src/fixture/` without its marker is still found
// (scripts/guard-negative-control.sh). Strings and long integers survive minification, so a
// fixture build that loses any of them fails its own check here. What this proves is that none of
// these strings is in the bundle, not that no line of fixture code is: code with none of them
// (a helper with no distinctive literal) is not detected.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { fixtureSentinels, holds } from "./fixture-sentinels.mjs";

const MARKERS = {
  "fixture sentinel": "dw-fixture-backend:U-07",
  "Tauri mock IPC": "Couldn't find callback id",
};

const [dir, mode] = process.argv.slice(2);
if (!dir || (mode !== "--production" && mode !== "--fixture")) {
  console.error("usage: check-bundle.mjs <dir> --production|--fixture");
  process.exit(2);
}

function* files(d) {
  for (const name of readdirSync(d)) {
    const p = join(d, name);
    if (statSync(p).isDirectory()) yield* files(p);
    else if (/\.(js|mjs|html|css|map)$/.test(name)) yield p;
  }
}

const texts = [...files(dir)].map((f) => [relative(dir, f), readFileSync(f, "utf8")]);
if (texts.length === 0) {
  console.error(`check-bundle: no JS, HTML or CSS files in ${dir}`);
  process.exit(1);
}
const where = (has) => texts.filter(([, text]) => has(text)).map(([name]) => name);

let ok = true;
for (const [name, marker] of Object.entries(MARKERS)) {
  const found = where((text) => text.includes(marker));
  if (mode === "--production" && found.length > 0) {
    console.error(`check-bundle: ${dir} is a production renderer but contains the ${name} in ${found.join(", ")}`);
    ok = false;
  }
  if (mode === "--fixture" && found.length === 0) {
    console.error(`check-bundle: ${dir} is a fixture build but has no ${name}: the check would prove nothing`);
    ok = false;
  }
}

const sentinels = await fixtureSentinels(join(dirname(fileURLToPath(import.meta.url)), ".."));
const shown = (literal) => (literal.startsWith("#") ? literal.slice(1) : JSON.stringify(literal));
const present = [];
const absent = [];
for (const [literal, source] of sentinels) {
  const found = where((text) => holds(text, literal));
  if (found.length > 0) present.push(`${shown(literal)} (${source}) in ${found.join(", ")}`);
  else absent.push(`${shown(literal)} (${source})`);
}
if (mode === "--production" && present.length > 0) {
  console.error(`check-bundle: ${dir} is a production renderer but contains ${present.length} fixture sentinel(s), literals of the fixture sources:`);
  for (const line of present) console.error(`  ${line}`);
  console.error("  (a collision with a dependency's own string: review it, then add it to GENERIC in scripts/fixture-sentinels.mjs)");
  ok = false;
}
if (mode === "--fixture" && absent.length > 0) {
  console.error(`check-bundle: ${dir} is a fixture build but lacks ${absent.length} fixture sentinel(s): the check would not find them`);
  for (const line of absent) console.error(`  ${line}`);
  ok = false;
}
if (!ok) process.exit(1);
console.log(
  mode === "--production"
    ? `check-bundle: ${dir}: ${texts.length} files, neither fixture marker and none of ${sentinels.size} fixture sentinels`
    : `check-bundle: ${dir}: ${texts.length} files, both fixture markers and all ${sentinels.size} fixture sentinels`,
);
