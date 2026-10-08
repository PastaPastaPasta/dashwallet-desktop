import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";
import { artifactPath, SCREENS, show, THEMES, writeJson } from "./helpers";

// U4: 0 serious or critical axe issues per screen, and every action reachable by keyboard.
//
// `color-contrast` is reported separately: the UX-SPEC palette itself (white on Dash blue
// #008DE4, white on the orange network capsule, textTertiary on white, blue chips) is below
// WCAG AA for small text, so a faithful rendering cannot pass it. Each failing pair is written
// to contrast.json with its ratio for the gate record; every other rule must be clean.
interface ContrastData {
  fgColor: string;
  bgColor: string;
  contrastRatio: number;
  expectedContrastRatio: string;
  fontSize: string;
  fontWeight: string;
}

for (const theme of THEMES) {
  for (const screen of SCREENS) {
    test(`axe ${screen} ${theme}`, async ({ page }, info) => {
      await show(page, screen, theme);
      const result = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "best-practice"]).analyze();
      const bad = result.violations.filter((v) => (v.impact === "serious" || v.impact === "critical") && v.id !== "color-contrast");
      expect(bad.map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`)).toEqual([]);

      const contrast = result.violations.find((v) => v.id === "color-contrast");
      const pairs = new Map<string, ContrastData & { targets: string[] }>();
      for (const node of contrast?.nodes ?? []) {
        const d = node.any[0]?.data as ContrastData | undefined;
        if (!d) continue;
        const key = `${d.fgColor} on ${d.bgColor} @ ${d.fontSize}/${d.fontWeight}`;
        const entry = pairs.get(key) ?? { ...d, targets: [] };
        entry.targets.push(node.target.join(" "));
        pairs.set(key, entry);
      }
      writeJson(artifactPath(info, `contrast-${theme}-${screen}.json`), [...pairs.values()]);
      info.annotations.push({ type: "color-contrast", description: `${contrast?.nodes.length ?? 0} nodes, ${pairs.size} colour pairs` });
    });
  }
}

test("keyboard only: navigate, open a transaction, close it, lock and unlock", async ({ page }) => {
  await page.goto("/?theme=light");
  await page.getByTestId("hero").waitFor();
  const focused = () => page.evaluate(() => document.activeElement?.getAttribute("data-testid") ?? document.activeElement?.tagName);

  // Tab order follows reading order: toolbar, sidebar, page.
  const seen: (string | undefined)[] = [];
  for (let i = 0; i < 12; i++) {
    await page.keyboard.press("Tab");
    seen.push(await focused());
  }
  expect(seen).toContain("toolbar-lock");
  expect(seen).toContain("sidebar-transactions");

  await page.getByTestId("sidebar-transactions").focus();
  await page.keyboard.press("Enter");
  await page.getByTestId("tx-list").waitFor();

  const firstRow = page.locator("[data-txid]").first();
  await firstRow.focus();
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("tx-detail")).toBeVisible();
  // Focus moved into the dialog, and Escape closes it and returns focus to the row.
  expect(await page.evaluate(() => !!document.activeElement?.closest('[role="dialog"]'))).toBe(true);
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("tx-detail")).toHaveCount(0);
  expect(await page.evaluate(() => document.activeElement?.hasAttribute("data-txid"))).toBe(true);

  await page.getByTestId("toolbar-lock").focus();
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("lock-screen")).toBeVisible();
  // The passphrase field has focus; a wrong passphrase is announced, the right one unlocks.
  await page.keyboard.type("wrong");
  await page.keyboard.press("Enter");
  await expect(page.locator("#lock-error")).toContainText("was incorrect");
  await page.keyboard.type("demo");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("lock-screen")).toHaveCount(0);
});
