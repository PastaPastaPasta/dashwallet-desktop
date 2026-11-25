import { expect, test, type Page } from "@playwright/test";
import { artifactPath, fixtureUrl, THEMES } from "./helpers";

/** Coins a dropped payment holds, set on the fixture's first wallet: `valued` duffs summed, and
 * `unvalued` coins of unknown value; `kind` patches the engine's facts about that wallet, and
 * `awaiting` says a held coin awaits a transaction that isn't final. */
async function hold(page: Page, valued: number, unvalued: number, kind: { watchOnly?: boolean; hasMnemonic?: boolean; legacyIssued?: boolean } = {}, awaiting = false) {
  await expect(page.getByTestId("hero-amount")).toBeVisible();
  await page.evaluate(
    ([valued, unvalued, kind, awaiting]) => {
      const f = window.__dwFixture!;
      const w = f.world.wallets[0]!;
      Object.assign(w.summary, kind);
      w.summary.balances = { ...w.summary.balances!, heldUnfinished: valued, heldUnvalued: unvalued, heldAwaiting: awaiting };
      f.emit({ kind: "eventsOverflowed" });
    },
    [valued, unvalued, kind, awaiting] as const,
  );
}

async function openWayOut(page: Page) {
  await page.getByTestId("hero-held-way-out").click();
  return page.getByTestId("held-way-out-text");
}

const STATES = [
  { name: "valued", valued: 150_000_000, unvalued: 0, line: "1.50 held by an unfinished payment" },
  { name: "at-least", valued: 150_000_000, unvalued: 2, line: "At least 1.50 held by an unfinished payment" },
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
      await hold(page, s.valued, s.unvalued);
      await expect(page.getByTestId("hero-held-text")).toHaveText(s.line);
      await page.getByTestId("hero-band").screenshot({ path: artifactPath(info, `${theme}-home-held-${s.name}.png`) });
      await expect(page.getByTestId("hero-held-way-out")).toHaveText("What can I do?");
    });
  }

  test(`the held line stays in discreet mode, its amount masked (${theme})`, async ({ page }, info) => {
    await page.goto(fixtureUrl("funded", theme));
    await hold(page, 150_000_000, 0);
    await page.getByTestId("hero-amount").click();
    await expect(page.getByTestId("hero-held-text")).toBeVisible();
    await expect(page.getByTestId("hero-held-text")).not.toContainText("1.50");
    await expect(page.getByTestId("hero-held-way-out")).toBeVisible();
    await page.getByTestId("hero-band").screenshot({ path: artifactPath(info, `${theme}-home-held-discreet.png`) });
  });

  test(`What can I do? restores a lone phrase wallet from the welcome screen (${theme})`, async ({ page }, info) => {
    await page.goto(fixtureUrl("funded", theme));
    await hold(page, 150_000_000, 0);
    const text = await openWayOut(page);
    await expect(text).toContainText("its BIP39 passphrase if the wallet was created with one. That isn't the passphrase you unlock Dash Wallet with");
    await expect(text).toContainText("Removing Main also unlocks any coins you locked with Lock unspent in Tools ▸ Coin Selection");
    await expect(text).toContainText("Otherwise Dash Wallet shows Welcome to Dash Wallet once Main is removed: choose Restore from recovery phrase and enter its phrase.");
    await expect(text).toContainText("under Scan the blockchain from, choose The beginning.");
    await expect(text).not.toContainText("extra wallet");
    await page.getByTestId("held-way-out").screenshot({ path: artifactPath(info, `${theme}-home-held-way-out-single.png`) });
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("held-way-out")).toHaveCount(0);
  });

  test(`What can I do? names the watch-only way out (${theme})`, async ({ page }, info) => {
    await page.goto(fixtureUrl("funded", theme));
    await hold(page, 0, 1, { watchOnly: true, hasMnemonic: false });
    const text = await openWayOut(page);
    await expect(text).toContainText("you'll remove Main and add it again as a watch-only wallet.");
    await expect(text).toContainText("Then, in Settings ▸ Wallet: add another wallet (a new one is fine), remove Main, and add it again with Add watch-only wallet… and that key.");
    await expect(text).toContainText("so don't create a payment from it until syncing finishes");
    await page.getByTestId("held-way-out").screenshot({ path: artifactPath(info, `${theme}-home-held-way-out-watch-only-single.png`) });
  });

  test(`What can I do? with two wallets open gives the several-wallets way out (${theme})`, async ({ page }, info) => {
    await page.goto(fixtureUrl("two-wallets", theme));
    await hold(page, 150_000_000, 0);
    const text = await openWayOut(page);
    await expect(text).toContainText("Then, in Settings ▸ Wallet, remove Main (if it asks you to open another wallet first, do that), and restore it with Add wallet… and its phrase.");
    await expect(text).not.toContainText("add another wallet");
    await page.getByTestId("held-way-out").screenshot({ path: artifactPath(info, `${theme}-home-held-way-out-several.png`) });
  });

  test(`What can I do? with two wallets open gives the several-wallets watch-only way out (${theme})`, async ({ page }, info) => {
    await page.goto(fixtureUrl("two-wallets", theme));
    await hold(page, 0, 1, { watchOnly: true, hasMnemonic: false });
    const text = await openWayOut(page);
    await expect(text).toContainText("Then, in Settings ▸ Wallet, remove Main and add it again with Add watch-only wallet… and that key.");
    await expect(text).not.toContainText("add another wallet");
    await page.getByTestId("held-way-out").screenshot({ path: artifactPath(info, `${theme}-home-held-way-out-watch-only-several.png`) });
  });

  test(`What can I do? waits while a held coin's transaction isn't final, then gives the way out (${theme})`, async ({ page }, info) => {
    await page.goto(fixtureUrl("funded", theme));
    await hold(page, 150_000_000, 0, {}, true);
    const text = await openWayOut(page);
    await expect(text).toHaveText(
      "Dash Wallet stopped this payment but still holds the coins it was using, and this version can't release them on its own. A transaction that uses some of these coins isn't final on the network yet. When it is, Dash Wallet shows here how to get back any coins the payment didn't spend. Until then, don't remove or restore Main.",
    );
    await page.getByTestId("held-way-out").screenshot({ path: artifactPath(info, `${theme}-home-held-waiting.png`) });
    await page.keyboard.press("Escape");
    await hold(page, 150_000_000, 0, {}, false);
    await expect(await openWayOut(page)).toContainText("restore it from its recovery phrase");
  });

  test(`What can I do? keeps a wallet added from a seed without a phrase (${theme})`, async ({ page }, info) => {
    await page.goto(fixtureUrl("funded", theme));
    await hold(page, 150_000_000, 0, { hasMnemonic: false });
    const text = await openWayOut(page);
    await expect(text).toContainText("Keep Main: don't remove it to get them back. It was added from a seed without a recovery phrase");
    await expect(text).not.toContainText("remove Main and");
    await page.getByTestId("held-way-out").screenshot({ path: artifactPath(info, `${theme}-home-held-keep-no-phrase.png`) });
  });

  test(`What can I do? keeps a wallet.dat import with legacy address ranges (${theme})`, async ({ page }, info) => {
    await page.goto(fixtureUrl("funded", theme));
    await hold(page, 150_000_000, 0, { legacyIssued: true });
    const text = await openWayOut(page);
    await expect(text).toContainText("Keep Main: don't remove it to get them back. It was added from a Dash Core wallet.dat that uses older addresses");
    await page.getByTestId("held-way-out").screenshot({ path: artifactPath(info, `${theme}-home-held-keep-legacy.png`) });
  });
}
