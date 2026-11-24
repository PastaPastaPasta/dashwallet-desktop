import { mkdirSync } from "node:fs";
import { expect, test, type Page } from "@playwright/test";
import { CLUB, HOME, OFFICE } from "../src/fixture/masternodes";
import { FIXTURE_PASSPHRASE } from "../src/fixture/scenarios";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors } from "./helpers";

// MG-10 S3: Change keys or payout (ProUpRegTx, signed with the owner key) and the operator
// explanation sheet on the fixture. DW_ROUTE_SHOTS=<dir> (scripts/e2e-docker.sh) also writes each
// state's screenshot there.

const shots = process.env.DW_ROUTE_SHOTS;

async function open(page: Page, scenario: string, theme: "light" | "dark" = "light", network?: "mainnet") {
  const errors = trackPageErrors(page);
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl(scenario, theme, network ? `&network=${network}` : ""));
  await page.getByTestId("toolbar").waitFor();
  await page.getByTestId("sidebar-masternodes").click();
  await expect(page.getByTestId("page-masternodes")).toBeVisible();
  return errors;
}

async function shot(page: Page, name: string, theme: string) {
  if (!shots) return;
  mkdirSync(shots, { recursive: true });
  await fontsReady(page);
  await page.screenshot({ path: `${shots}/${name}-${theme}.png` });
}

const sheet = (page: Page) => page.getByTestId("mn-change-keys-sheet");
const PAYOUT = "yNewPayoutMn7qR2vWx9KfT4Mn7qR2vWx9";

for (const theme of THEMES) {
  test(`change keys ${theme}: the PoSe warning, the recorded payout, then send; 0 serious axe issues`, async ({ page }) => {
    const errors = await open(page, "mn-list", theme);
    await page.getByTestId(`mn-row-${HOME}`).click();
    await shot(page, "pane-home", theme);
    await page.getByTestId("mn-change-keys").click();
    await expect(sheet(page)).toBeVisible();
    // Q4: the recorded payout is preselected, with its set-back warning; both keys kept.
    await expect(page.getByTestId("mn-change-payout-recorded").locator("input")).toBeChecked();
    await expect(sheet(page)).toContainText("If the payout was changed elsewhere, this sets it back.");
    await expect(page.getByTestId("mn-change-nothing")).toBeVisible();
    await expect(sheet(page).locator("fieldset, legend")).toHaveCount(0);
    for (const name of ["Operator key", "Voting key", "Payouts go to"]) await expect(sheet(page).getByRole("radiogroup", { name })).toBeVisible();
    await expect(page.getByTestId("mn-change-send")).toBeDisabled();
    await shot(page, "change-keys-open", theme);
    await page.getByTestId("mn-change-operator-keychain").click();
    await expect(page.getByTestId("mn-change-pose")).toContainText("Changing the operator key immediately PoSe-bans the masternode.");
    await expect(page.getByTestId("mn-change-review-operator")).toContainText("Changes");
    await expect(page.getByTestId("mn-change-signer")).toHaveText("Signed with its owner key from this wallet.");
    await fontsReady(page);
    expect(await seriousAxeViolations(page)).toEqual([]);
    await shot(page, "change-keys-operator", theme);
    await page.getByTestId("mn-change-passphrase").fill(FIXTURE_PASSPHRASE);
    await page.getByTestId("mn-change-send").click();
    await expect(page.getByTestId("mn-change-keys-sent")).toContainText("Change sent. It applies once it's in a block.");
    await shot(page, "change-keys-sent", theme);
    await page.getByTestId("mn-change-keys-done").click();
    // The next change waits for this one to reach the list (MG-04 D49).
    await page.getByTestId("mn-change-keys").click();
    await expect(page.getByTestId("mn-change-preview-error")).toContainText("hasn't reached the masternode list yet");
    await shot(page, "change-keys-pending", theme);
    expect(errors).toEqual([]);
  });

  test(`operator sheet ${theme}: self-run and hosted custody`, async ({ page }) => {
    await open(page, "mn-list", theme);
    await page.getByTestId(`mn-row-${HOME}`).click();
    await page.getByTestId("mn-more").click();
    await page.getByTestId("mn-more-operator").click();
    await expect(page.getByTestId("mn-operator-intro")).toContainText("This wallet derives your operator key");
    await expect(page.getByTestId("mn-operator-replace")).toBeVisible();
    await expect(page.getByTestId("mn-operator-end")).toBeVisible();
    await shot(page, "operator-self-run", theme);
    await page.getByTestId("mn-operator-close").click();
    await page.getByTestId(`mn-row-${CLUB}`).click();
    await page.getByTestId("mn-more").click();
    await page.getByTestId("mn-more-operator").click();
    await expect(page.getByTestId("mn-operator-intro")).toContainText("Your operator signs these with the operator key");
    // Its owner key is added but the wallet lacks the registration: the sheet says why it can't replace the operator.
    await expect(page.getByTestId("mn-operator-reason")).toContainText("Changing keys needs this masternode's registration");
    await expect(page.getByTestId("mn-operator-change-keys")).toHaveCount(0);
    await shot(page, "operator-hosted", theme);
  });
}

test("a given operator key and another payout address are signed as reviewed; a wrong key is named", async ({ page }) => {
  await open(page, "mn-list");
  await page.getByTestId(`mn-row-${OFFICE}`).click();
  await page.getByTestId("mn-change-keys").click();
  await page.getByTestId("mn-change-operator-given").click();
  await page.getByTestId("mn-change-operator-key").fill("abc");
  await expect(page.getByTestId("mn-change-operator-key-error")).toHaveText("Enter the 96-character hex public key your operator gave you.");
  await page.getByTestId("mn-change-operator-key").fill("ab".repeat(48));
  await expect(page.getByTestId("mn-change-operator-key-error")).toHaveCount(0);
  await expect(page.getByTestId("mn-change-pose")).toBeVisible();
  await page.getByTestId("mn-change-payout-address").click();
  await expect(page.getByTestId("mn-change-send")).toBeDisabled();
  await page.getByTestId("mn-change-payout-input").fill(PAYOUT);
  await expect(page.getByTestId("mn-change-review-payout")).toContainText("Changes");
  await page.getByTestId("mn-change-passphrase").fill(FIXTURE_PASSPHRASE);
  await page.getByTestId("mn-change-send").click();
  await expect(page.getByTestId("mn-change-keys-sent")).toBeVisible();
  const terms = await page.evaluate(() => window.__dwFixture!.world.masternodes.operations.find((o) => o.kind === "update_registrar")!.terms);
  expect(terms).toMatchObject({ operatorPublicKey: "ab".repeat(48), payoutAddress: PAYOUT });
});

for (const theme of THEMES)
  test(`an update that didn't go out ${theme}: send it again, or discard it`, async ({ page }) => {
    const errors = await open(page, "mn-update-not-sent", theme);
    await page.getByTestId(`mn-row-${HOME}`).click();
    await page.getByTestId("mn-change-keys").click();
    await page.getByTestId("mn-change-operator-keychain").click();
    await page.getByTestId("mn-change-passphrase").fill(FIXTURE_PASSPHRASE);
    await page.getByTestId("mn-change-send").click();
    await expect(page.getByTestId("mn-change-unsent")).toContainText("This change didn't go out");
    await expect(page.getByTestId("mn-change-unsent")).toContainText("The wallet had no connected peers.");
    await fontsReady(page);
    expect(await seriousAxeViolations(page)).toEqual([]);
    await shot(page, "change-keys-not-sent", theme);
    // Closing and reopening comes back to it: the saved row blocks a new change until it's sent or discarded.
    await page.getByTestId("mn-change-keys-sheet").getByRole("button", { name: "Cancel" }).click();
    await page.getByTestId("mn-change-keys").click();
    await expect(page.getByTestId("mn-change-unsent")).toBeVisible();
    await page.getByTestId("mn-change-unsent-passphrase").fill(FIXTURE_PASSPHRASE);
    await page.getByTestId("mn-change-unsent-resend").click();
    await expect(page.getByTestId("mn-change-keys-sent")).toBeVisible();
    expect(errors).toEqual([]);
  });

test("discarding an update that didn't go out frees the masternode for another change", async ({ page }) => {
  await open(page, "mn-update-not-sent");
  await page.getByTestId(`mn-row-${HOME}`).click();
  await page.getByTestId("mn-change-keys").click();
  await page.getByTestId("mn-change-voting-keychain").click();
  await page.getByTestId("mn-change-passphrase").fill(FIXTURE_PASSPHRASE);
  await page.getByTestId("mn-change-send").click();
  await page.getByTestId("mn-change-unsent-discard").click();
  await expect(page.getByTestId("mn-change-keys-discarded")).toContainText("Discarded. Nothing was sent.");
  await page.getByTestId("mn-change-keys-done").click();
  await page.getByTestId("mn-change-keys").click();
  await expect(page.getByTestId("mn-change-review-voting")).toBeVisible();
});

test("mainnet: Change keys can be reviewed, and nothing is sent", async ({ page }) => {
  const errors = await open(page, "mn-list", "light", "mainnet");
  await page.getByTestId(`mn-row-${HOME}`).click();
  await page.getByTestId("mn-change-keys").click();
  await expect(sheet(page)).toBeVisible();
  await page.getByTestId("mn-change-operator-keychain").click();
  await expect(page.getByTestId("mn-change-pose")).toBeVisible();
  await expect(page.getByTestId("mn-change-held")).toHaveText("You can review this change, but this version can't send it yet. Nothing has been sent.");
  await expect(page.getByTestId("mn-change-send")).toHaveCount(0);
  await expect(page.getByTestId("mn-change-passphrase")).toHaveCount(0);
  await sheet(page).locator("form").evaluate((f: HTMLFormElement) => f.requestSubmit());
  await expect(sheet(page)).toBeVisible();
  await expect(page.getByTestId("mn-change-keys-sent")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("no Change keys for a tracked node without its registration: the reason line instead", async ({ page }) => {
  await open(page, "mn-list");
  await page.getByTestId(`mn-row-${CLUB}`).click();
  await expect(page.getByTestId("mn-registrar-line")).toBeVisible();
  await expect(page.getByTestId("mn-change-keys")).toHaveCount(0);
});
