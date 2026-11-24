import { expect, test, type Page } from "@playwright/test";
import { TEST_VECTOR_WIF } from "../src/fixture/masternodes";
import { FIXTURE_WALLET_ID } from "../src/fixture/scenarios";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { artifactPath, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// Masternode keys (IOS-083, MG-10 §1.5): opened from Masternodes ▸ More, segments per role, "Show
// more" paging, rows matched to known masternodes, and a reveal behind the passphrase that is
// dropped on hide.

const open = async (page: Page, theme: Theme, scenario = "mn-list", before?: () => Promise<unknown>) => {
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  if (before) await before();
  await page.getByTestId("sidebar-masternodes").click();
  await expect(page.getByTestId("page-masternodes")).toBeVisible();
  await page.getByTestId("mn-page-more").click();
  await page.getByRole("menuitem", { name: "Masternode keys…" }).click();
  await expect(page.getByTestId("mn-keys")).toBeVisible();
};

test("Masternode keys lists each role, pages, and reveals one secret behind the passphrase", async ({ page, browserName }) => {
  const errors = trackPageErrors(page);
  await open(page, "light");
  const sheet = page.getByTestId("mn-keys");
  await expect(sheet.getByTestId("mn-keys-list").locator("li")).toHaveCount(20);
  await expect(sheet.getByTestId("mn-key-0")).toContainText("Keypair 1");
  await expect(sheet.getByTestId("mn-key-1-match")).toHaveText("Matched to Office node · 203.0.113.9:19999");
  await expect(sheet.getByTestId("mn-key-2-match")).toHaveText("Not matched to a known masternode");

  await sheet.getByTestId("mn-keys-role-operator").click();
  await expect(sheet.getByTestId("mn-keys-list").locator("li")).toHaveCount(20);
  await sheet.getByTestId("mn-keys-more").click();
  await expect(sheet.getByTestId("mn-keys-list").locator("li")).toHaveCount(25);
  await expect(sheet.getByTestId("mn-keys-more")).toHaveCount(0);

  await sheet.getByTestId("mn-keys-role-owner").click();
  await sheet.getByTestId("mn-key-0-show").click();
  await expect(sheet.getByTestId("mn-key-0-address")).toBeVisible();
  await expect(sheet.getByTestId("mn-key-0-hidden")).toBeVisible();
  await sheet.getByTestId("mn-key-0-reveal").click();
  const field = sheet.getByTestId("mn-key-0-passphrase-form").getByLabel("Passphrase");
  await field.fill("demo");
  await field.press("Enter");
  await expect(sheet.getByTestId("mn-key-secret-wif")).toContainText(TEST_VECTOR_WIF);
  // An owner secret refuses copying; an operator secret, which goes to the server, copies.
  await expect(sheet.getByTestId("mn-key-secret-wif").getByRole("button")).toHaveCount(0);
  await expect(sheet.getByTestId("mn-key-no-copy")).toContainText("This key can't be copied");
  await expect(sheet.getByTestId("mn-key-copy-warning")).toHaveCount(0);
  // A copy or cut on the revealed owner key is refused, in every engine.
  const wif = sheet.getByTestId("mn-key-secret-wif").locator(".mg-key-no-copy");
  for (const type of ["copy", "cut"])
    expect(await wif.evaluate((el, type) => el.dispatchEvent(new ClipboardEvent(type, { bubbles: true, cancelable: true })), type)).toBe(false);
  // Ctrl/Cmd-C on the selected key leaves the clipboard as it was. WebKit grants no clipboard-read
  // permission, so the system clipboard is read on Chromium only.
  if (browserName === "chromium") {
    await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
    await page.evaluate(() => navigator.clipboard.writeText("before"));
    await wif.evaluate((el) => {
      const range = document.createRange();
      range.selectNodeContents(el);
      const sel = window.getSelection()!;
      sel.removeAllRanges();
      sel.addRange(range);
    });
    await page.keyboard.press("ControlOrMeta+C");
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe("before");
  }
  await sheet.getByTestId("mn-key-0-hide-secret").click();
  await expect(sheet.getByTestId("mn-key-secret-wif")).toHaveCount(0);
  await expect(sheet.getByTestId("mn-key-0-hidden")).toBeVisible();
  // Shown again, then the window loses focus: the secret hides.
  await sheet.getByTestId("mn-key-0-reveal").click();
  const again = sheet.getByTestId("mn-key-0-passphrase-form").getByLabel("Passphrase");
  await again.fill("demo");
  await again.press("Enter");
  await expect(sheet.getByTestId("mn-key-secret-wif")).toBeVisible();
  await page.evaluate(() => window.dispatchEvent(new Event("blur")));
  await expect(sheet.getByTestId("mn-key-secret-wif")).toHaveCount(0);
  await sheet.getByTestId("mn-keys-role-operator").click();
  await sheet.getByTestId("mn-key-0-show").click();
  await sheet.getByTestId("mn-key-0-reveal").click();
  const op = sheet.getByTestId("mn-key-0-passphrase-form").getByLabel("Passphrase");
  await op.fill("demo");
  await op.press("Enter");
  await expect(sheet.getByTestId("mn-key-secret-hex").getByRole("button")).toHaveCount(1);
  await expect(sheet.getByTestId("mn-key-copy-warning")).toContainText("Paste this only into your masternode's configuration.");
  await expect(sheet.getByTestId("mn-key-no-copy")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("a key says Checking… until the known masternodes are read, and never Not matched before", async ({ page }) => {
  let release: Awaited<ReturnType<typeof page.evaluateHandle<() => void>>> | null = null;
  await open(page, "light", "mn-list", async () => {
    release = await page.evaluateHandle(() => window.__dwFixture!.hold("mn_masternode"));
  });
  const sheet = page.getByTestId("mn-keys");
  await expect(sheet.getByTestId("mn-key-1-match")).toHaveText("Checking…");
  await expect(sheet.getByTestId("mn-key-2-match")).toHaveText("Checking…");
  await release!.evaluate((r) => r());
  await expect(sheet.getByTestId("mn-key-1-match")).toHaveText("Matched to Office node · 203.0.113.9:19999");
  await expect(sheet.getByTestId("mn-key-2-match")).toHaveText("Not matched to a known masternode");
});

test("a failed read says Couldn't check, with Try again", async ({ page }) => {
  await open(page, "light", "mn-list-failed");
  const sheet = page.getByTestId("mn-keys");
  await expect(sheet.getByTestId("mn-key-2-match")).toHaveText("Couldn't check");
  await expect(sheet.getByTestId("mn-keys-check-failed")).toContainText("Couldn't check which known masternodes these keys belong to.");
  await page.evaluate(() => (window.__dwFixture!.world.masternodes.listFails = false));
  await sheet.getByTestId("mn-keys-check-retry").click();
  await expect(sheet.getByTestId("mn-key-1-match")).toHaveText("Matched to Office node · 203.0.113.9:19999");
  await expect(sheet.getByTestId("mn-keys-check-failed")).toHaveCount(0);
});

test("a refresh that fails after a match says Couldn't check, not the old match", async ({ page }) => {
  await open(page, "light");
  const sheet = page.getByTestId("mn-keys");
  await expect(sheet.getByTestId("mn-key-1-match")).toHaveText("Matched to Office node · 203.0.113.9:19999");
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.world.masternodes.detailFails = true;
    f.emit({ kind: "vaultLockState", network: f.world.launch.network.dirName, state: "unlocked" });
  });
  await expect(sheet.getByTestId("mn-key-1-match")).toHaveText("Couldn't check");
  await expect(sheet.getByTestId("mn-keys-check-failed")).toBeVisible();
  await page.evaluate(() => (window.__dwFixture!.world.masternodes.detailFails = false));
  await sheet.getByTestId("mn-keys-check-retry").click();
  await expect(sheet.getByTestId("mn-key-1-match")).toHaveText("Matched to Office node · 203.0.113.9:19999");
});

test("a key page that fails to load offers Try again and keeps the rows already in", async ({ page }) => {
  await open(page, "light");
  const sheet = page.getByTestId("mn-keys");
  await sheet.getByTestId("mn-keys-role-operator").click();
  await expect(sheet.getByTestId("mn-keys-list").locator("li")).toHaveCount(20);
  await page.evaluate(() => (window.__dwFixture!.world.masternodes.keyPagesFail = true));
  await sheet.getByTestId("mn-keys-more").click();
  await expect(sheet.getByTestId("mn-keys-failed")).toBeVisible();
  await expect(sheet.getByTestId("mn-keys-list").locator("li")).toHaveCount(20);
  await page.evaluate(() => (window.__dwFixture!.world.masternodes.keyPagesFail = false));
  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("mn_keys"));
  // By keyboard: while the page is read again, the error and an inert Try again stay, and keep focus.
  const retry = sheet.getByTestId("mn-keys-retry");
  await retry.focus();
  await page.keyboard.press("Enter");
  await expect(retry).toHaveAttribute("aria-disabled", "true");
  await expect(retry).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(sheet.getByTestId("mn-keys-list").locator("li")).toHaveCount(20);
  await release.evaluate((r) => r());
  await expect(sheet.getByTestId("mn-keys-list").locator("li")).toHaveCount(25);
  await expect(sheet.getByTestId("mn-keys-failed")).toHaveCount(0);
  // Focus moves into the list, not to the page.
  await expect(sheet.getByTestId("mn-keys-list")).toBeFocused();
  // Another role read for the first time shows no error left from the retry.
  const held = await page.evaluateHandle(() => window.__dwFixture!.hold("mn_keys"));
  await sheet.getByTestId("mn-keys-role-platform_node").click();
  await expect(sheet.getByTestId("mn-keys-failed")).toHaveCount(0);
  await held.evaluate((r) => r());
  await expect(sheet.getByTestId("mn-keys-failed")).toHaveCount(0);
});

test("switching role while a key page retry runs shows no error from the other role", async ({ page }) => {
  await open(page, "light");
  const sheet = page.getByTestId("mn-keys");
  await sheet.getByTestId("mn-keys-role-operator").click();
  await expect(sheet.getByTestId("mn-keys-list").locator("li")).toHaveCount(20);
  await page.evaluate(() => (window.__dwFixture!.world.masternodes.keyPagesFail = true));
  await sheet.getByTestId("mn-keys-more").click();
  await expect(sheet.getByTestId("mn-keys-failed")).toBeVisible();
  await page.evaluate(() => (window.__dwFixture!.world.masternodes.keyPagesFail = false));
  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("mn_keys"));
  await sheet.getByTestId("mn-keys-retry").click();
  await sheet.getByTestId("mn-keys-role-platform_node").click();
  await expect(sheet.getByTestId("mn-keys-failed")).toHaveCount(0);
  await expect(sheet.getByTestId("mn-keys-role-platform_node")).toBeFocused();
  await release.evaluate((r) => r());
  await expect(sheet.getByTestId("mn-keys-list").locator("li")).toHaveCount(20);
  await expect(sheet.getByTestId("mn-keys-failed")).toHaveCount(0);
  await expect(sheet.getByTestId("mn-keys-role-platform_node")).toBeFocused();
});

test("a key page retried offline waits with its error and an inert Try again, and loads on reconnect", async ({ page, context }) => {
  await open(page, "light");
  const sheet = page.getByTestId("mn-keys");
  await sheet.getByTestId("mn-keys-role-operator").click();
  await expect(sheet.getByTestId("mn-keys-list").locator("li")).toHaveCount(20);
  await page.evaluate(() => (window.__dwFixture!.world.masternodes.keyPagesFail = true));
  await sheet.getByTestId("mn-keys-more").click();
  await expect(sheet.getByTestId("mn-keys-failed")).toBeVisible();
  await page.evaluate(() => (window.__dwFixture!.world.masternodes.keyPagesFail = false));
  await context.setOffline(true);
  await sheet.getByTestId("mn-keys-retry").click();
  await expect(sheet.getByTestId("mn-keys-failed")).toBeVisible();
  await expect(sheet.getByTestId("mn-keys-retry")).toHaveAttribute("aria-disabled", "true");
  await context.setOffline(false);
  await expect(sheet.getByTestId("mn-keys-list").locator("li")).toHaveCount(25);
  await expect(sheet.getByTestId("mn-keys-failed")).toHaveCount(0);
});

test("a wallet without its seed lists public keys and offers no Reveal", async ({ page }) => {
  await open(page, "light", "mn-watchonly", () =>
    page.evaluate((id) => (window.__dwFixture!.world.masternodes.keysWithoutSeed = [id]), FIXTURE_WALLET_ID),
  );
  const sheet = page.getByTestId("mn-keys");
  await expect(sheet.getByTestId("mn-keys-no-seed")).toContainText("private keys are no longer in Dash Wallet");
  await sheet.getByTestId("mn-key-0-show").click();
  await expect(sheet.getByTestId("mn-key-0-address")).toBeVisible();
  await expect(sheet.getByTestId("mn-key-0-no-seed")).toBeVisible();
  await expect(sheet.getByTestId("mn-key-0-reveal")).toHaveCount(0);
});

for (const theme of THEMES) {
  test(`Masternode keys, ${theme}`, async ({ page }, info) => {
    await open(page, theme);
    const sheet = page.getByTestId("mn-keys");
    await sheet.getByTestId("mn-keys-role-operator").click();
    await sheet.getByTestId("mn-key-0-show").click();
    await expect(sheet.getByTestId("mn-key-0-public-key")).toBeVisible();
    await fontsReady(page);
    await page.screenshot({ path: artifactPath(info, `mn-keys-operator-${theme}.png`) });
  });
}
