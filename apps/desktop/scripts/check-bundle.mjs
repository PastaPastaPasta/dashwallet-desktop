#!/usr/bin/env node
// Proves a built renderer does (or does not) contain the fixture backend (U-07).
//
//   node scripts/check-bundle.mjs dist --production    no fixture code: `pnpm build` runs this,
//                                                      so every `tauri build` does too
//   node scripts/check-bundle.mjs dist-fixture --fixture   the positive control: the same
//                                                      markers are found in a fixture build
//
// The markers: the fixture's sentinel string (src/fixture/index.ts `FIXTURE_SENTINEL`) and a
// string only Tauri's mock IPC module (`@tauri-apps/api/mocks`) contains. Strings survive
// minification, so a fixture build that loses them fails its own check here.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";

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

const found = new Map(Object.keys(MARKERS).map((k) => [k, []]));
let scanned = 0;
for (const f of files(dir)) {
  scanned += 1;
  const text = readFileSync(f, "utf8");
  for (const [name, marker] of Object.entries(MARKERS)) if (text.includes(marker)) found.get(name).push(relative(dir, f));
}
if (scanned === 0) {
  console.error(`check-bundle: no JS, HTML or CSS files in ${dir}`);
  process.exit(1);
}

let ok = true;
for (const [name, where] of found) {
  if (mode === "--production" && where.length > 0) {
    console.error(`check-bundle: ${dir} is a production renderer but contains the ${name} in ${where.join(", ")}`);
    ok = false;
  }
  if (mode === "--fixture" && where.length === 0) {
    console.error(`check-bundle: ${dir} is a fixture build but has no ${name}: the check would prove nothing`);
    ok = false;
  }
}
if (!ok) process.exit(1);
console.log(`check-bundle: ${dir}: ${scanned} files, ${mode === "--production" ? "no fixture code" : "fixture backend present"}`);
