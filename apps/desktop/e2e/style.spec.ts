import { expect, test } from "@playwright/test";
import type { Report } from "../src/app/selftest";
import { artifactPath, writeJson } from "./helpers";

// U3(a) computed-style checks and U3(c) boxes, through the same in-page routine `--selftest` runs.
test("UX-SPEC §2 checklist holds in every screen and appearance", async ({ page }, info) => {
  await page.goto("/");
  await page.getByTestId("hero").waitFor();
  const report: Report = await page.evaluate(() => window.__dwSelftest!());
  writeJson(artifactPath(info, "selftest.json"), report);
  const failed = Object.entries(report.screens).flatMap(([screen, r]) => r.checks.filter((c) => c.pass !== true).map((c) => ({ screen, ...c })));
  expect(failed).toEqual([]);
  expect(report.interFontLoaded).toBe(true);
  expect(Object.keys(report.screens).sort()).toEqual(
    ["dark/overview", "dark/transactions", "dark/txDetail", "dark/unlock", "light/overview", "light/transactions", "light/txDetail", "light/unlock"],
  );
});
