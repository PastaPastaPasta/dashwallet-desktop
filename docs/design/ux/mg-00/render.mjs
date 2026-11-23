// Renders every frame of the MG-00 boards (build.py's output) in light and dark into
// docs/design/screenshots/mg-00/<frame>-<theme>.png, at the Tauri window's default size and
// deviceScaleFactor 1 (as apps/desktop/playwright.config.ts).
//
// Usage, after `pnpm install` in apps/desktop and `python3 -I docs/design/ux/mg-00/build.py`:
//   node docs/design/ux/mg-00/render.mjs [board …]

import { mkdirSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../../../..");
const require = createRequire(resolve(repo, "apps/desktop/package.json"));
const { chromium } = require("@playwright/test");

const out = resolve(repo, "docs/design/screenshots/mg-00");
mkdirSync(out, { recursive: true });
const boards = process.argv.slice(2).length
  ? process.argv.slice(2)
  : ["masternodes", "governance", "registration", "maintenance", "shared"];

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1328, height: 900 }, deviceScaleFactor: 1, locale: "en-US" });
const errors = [];
page.on("pageerror", (e) => errors.push(e.message));
for (const board of boards) {
  const file = resolve(here, `${board}.html`);
  const ids = [...readFileSync(file, "utf8").matchAll(/<section class="mock-frame" id="([^"]+)"/g)].map((m) => m[1])
    .filter((id) => !id.endsWith("-behind")); // backdrops for sheet frames, not mocks of their own
  for (const theme of ["light", "dark"]) {
    for (const id of ids) {
      await page.goto(`${pathToFileURL(file).href}?theme=${theme}#${id}`);
      await page.evaluate(() => document.fonts.ready);
      await page.locator(`#${id} .mock-window`).screenshot({ path: resolve(out, `${id}-${theme}.png`) });
      console.log(`${id}-${theme}.png`);
    }
  }
}
await browser.close();
if (errors.length) {
  console.error(errors.join("\n"));
  process.exit(1);
}
