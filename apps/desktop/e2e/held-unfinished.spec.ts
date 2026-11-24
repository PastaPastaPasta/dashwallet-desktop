import { expect, test, type Page } from "@playwright/test";
import { artifactPath, fixtureUrl, THEMES } from "./helpers";

/** Coins a dropped payment holds, set on the fixture's first wallet: `valued` duffs summed, and
 * `unvalued` coins of unknown value. */
async function hold(page: Page, valued: number, unvalued: number, watchOnly = false) {
  await page.evaluate(
    ([valued, unvalued, watchOnly]) => {
      const f = window.__dwFixture!;
      const w = f.world.wallets[0]!;
      w.summary.watchOnly = watchOnly;
      w.summary.balances = { ...w.summary.balances!, heldUnfinished: valued, heldUnvalued: unvalued };
      f.emit({ kind: "eventsOverflowed" });
    },
    [valued, unvalued, watchOnly] as const,
  );
}

const STATES = [
  { name: "valued", valued: 150_000_000, unvalued: 0, line: "1.50 tDASH held by an unfinished payment" },
  { name: "at-least", valued: 150_000_000, unvalued: 2, line: "At least 1.50 tDASH held by an unfinished payment" },
  { name: "unvalued-one", valued: 0, unvalued: 1, line: "A coin held by an unfinished payment" },
  { name: "unvalued", valued: 0, unvalued: 3, line: "3 coins held by an unfinished payment" },
] as const;

for (const theme of THEMES) {
  test(`Home has no held line while nothing is held (${theme})`, async ({ page }) => {
    await page.goto(fixtureUrl("funded", theme));
    await expect(page.getByTestId("hero-amount")).toBeVisible();
    await expect(page.getByTestId("hero-held")).toHaveCount(0);
  });

  for (const s of STATES) {
    test(`Home shows the held line, ${s.name} (${theme})`, async ({ page }, info) => {
      await page.goto(fixtureUrl("funded", theme));
      await expect(page.getByTestId("hero-amount")).toBeVisible();
      await hold(page, s.valued, s.unvalued);
      await expect(page.getByTestId("hero-held-text")).toHaveText(s.line);
      await page.getByTestId("hero-band").screenshot({ path: artifactPath(info, `${theme}-home-held-${s.name}.png`) });
      await expect(page.getByTestId("hero-held-way-out")).toHaveText("What can I do?");
    });
  }

  test(`the held line stays in discreet mode, its amount masked (${theme})`, async ({ page }) => {
    await page.goto(fixtureUrl("funded", theme));
    await hold(page, 150_000_000, 0);
    await page.getByTestId("hero-amount").click();
    await expect(page.getByTestId("hero-held-text")).toBeVisible();
    await expect(page.getByTestId("hero-held-text")).not.toContainText("1.50");
    await expect(page.getByTestId("hero-held-way-out")).toBeVisible();
  });

  test(`What can I do? opens the way out for a lone phrase wallet (${theme})`, async ({ page }, info) => {
    await page.goto(fixtureUrl("funded", theme));
    await hold(page, 150_000_000, 0);
    await page.getByTestId("hero-held-way-out").click();
    const text = page.getByTestId("held-way-out-text");
    await expect(text).toContainText("Then, in Settings ▸ Wallet: add another wallet (a new one is fine), remove Main, and restore it with Add wallet… and its phrase.");
    await expect(text).toContainText("On Restore options, under Scan the blockchain from, choose The beginning.");
    await expect(text).toContainText("You can remove the extra wallet afterwards.");
    await page.getByTestId("held-way-out").screenshot({ path: artifactPath(info, `${theme}-home-held-way-out-single.png`) });
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("held-way-out")).toHaveCount(0);
  });

  test(`What can I do? names the watch-only way out (${theme})`, async ({ page }, info) => {
    await page.goto(fixtureUrl("funded", theme));
    await hold(page, 0, 1, true);
    await page.getByTestId("hero-held-way-out").click();
    const text = page.getByTestId("held-way-out-text");
    await expect(text).toContainText("you'll remove Main and add it again as a watch-only wallet.");
    await expect(text).toContainText("Then, in Settings ▸ Wallet: add another wallet (a new one is fine), remove Main, and add it again with Add watch-only wallet… and that key.");
    await page.getByTestId("held-way-out").screenshot({ path: artifactPath(info, `${theme}-home-held-way-out-watch-only-single.png`) });
  });

  test(`What can I do? with two wallets open gives the several-wallets way out (${theme})`, async ({ page }) => {
    await page.goto(fixtureUrl("two-wallets", theme));
    await hold(page, 150_000_000, 0);
    await page.getByTestId("hero-held-way-out").click();
    const text = page.getByTestId("held-way-out-text");
    await expect(text).toContainText("Then, in Settings ▸ Wallet, remove Main and restore it with Add wallet… and its phrase.");
    await expect(text).not.toContainText("add another wallet");
  });
}
