import { expect, test, type Page } from "@playwright/test";
import type { ShellCommand } from "../src/api/generated/ShellCommand";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors } from "./helpers";

// MG-09: Masternodes ▸ Governance on the fixture: the list, the pane, the vote sheet, Mine, and
// the states (sync off, syncing, failed, no voting key, watch-only), in light and dark with 0
// serious axe issues. The fixture's chain times are FIXTURE_NOW's, so the clock is pinned to it.

const command = (page: Page, c: ShellCommand) => page.evaluate((c) => window.__dwFixture!.shellCommand(c), c);

async function open(page: Page, scenario: string, theme: "light" | "dark" = "light") {
  const errors = trackPageErrors(page);
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  await page.getByTestId("sidebar-masternodes").click();
  await expect(page.getByTestId("page-masternodes")).toBeVisible();
  return errors;
}

for (const theme of THEMES) {
  test(`governance list ${theme}: clock, budget, rows and pane; 0 serious axe issues`, async ({ page }) => {
    const errors = await open(page, "governance", theme);
    await expect(page.getByTestId("governance-clock-title")).toHaveText("About 1 day 7 hours left to vote for the Oct 10 payout");
    await expect(page.getByTestId("governance-budget")).toContainText("goes to 1 passing proposal");
    await expect(page.getByTestId("governance-filter-all")).toHaveText("All 5");
    await expect(page.getByTestId("proposal-row-dash-incubator-nov")).toHaveAttribute("aria-current", "true");
    await expect(page.getByTestId("proposal-row-alt-wallet-hardening")).toContainText("Too close to call");
    await expect(page.getByTestId("proposal-pane-chip")).toHaveText("Passing");
    await fontsReady(page);
    expect(await seriousAxeViolations(page)).toEqual([]);
    expect(errors).toEqual([]);
  });

  test(`vote sheet ${theme}: two masternodes, one refused with the engine's reason`, async ({ page }) => {
    const errors = await open(page, "governance-vote-refused", theme);
    await page.getByTestId("proposal-row-dashcon-2027-venue").click();
    await page.getByTestId("proposal-vote").click();
    const sheet = page.getByTestId("vote-sheet");
    await expect(sheet).toBeVisible();
    // The banned masternode is listed, disabled with its reason; the other two start checked.
    await expect(sheet.getByTestId("vote-node-c3c3c3c3")).toContainText("PoSe banned");
    await expect(page.getByTestId("vote-privacy")).toBeVisible();
    await expect(page.getByTestId("vote-selection")).toHaveText("2 masternodes · weight 5 · no fee");
    await page.getByTestId("vote-outcome-yes").click();
    await fontsReady(page);
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("vote-passphrase").fill("demo");
    await page.getByTestId("vote-send").click();
    await expect(page.getByTestId("vote-result-title")).toHaveText("1 of 2 votes sent");
    await expect(page.getByTestId("vote-result-b2b2b2b2")).toContainText("Its voting key changed");
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("vote-done").click();
    await expect(sheet).toBeHidden();
    // The tally follows the cast vote; the row says the wallet's vote.
    await expect(page.getByTestId("proposal-row-dashcon-2027-venue")).toContainText("You: 1 of 2 seen voting");
    expect(errors).toEqual([]);
  });
}

test("a wrong passphrase keeps the vote sheet open and says so", async ({ page }) => {
  await open(page, "governance");
  await page.getByTestId("proposal-row-dashcon-2027-venue").click();
  await page.getByTestId("proposal-vote").click();
  await page.getByTestId("vote-outcome-no").click();
  await page.getByTestId("vote-passphrase").fill("wrong");
  await page.getByTestId("vote-send").click();
  await expect(page.getByTestId("vote-error")).toHaveText("The passphrase you entered was incorrect.");
  await expect(page.getByTestId("vote-result")).toHaveCount(0);
});

test("a vote from Needs my vote keeps its result after the row leaves the list (D31)", async ({ page }) => {
  await open(page, "governance");
  await page.getByTestId("governance-filter-needsVote").click();
  const first = page.getByTestId("proposal-list").locator("li").first();
  const name = (await first.locator(".mg-row-title").textContent())!;
  await first.locator("button").click();
  await page.getByTestId("proposal-vote").click();
  await page.getByTestId("vote-outcome-yes").click();
  await page.getByTestId("vote-passphrase").fill("demo");
  await page.getByTestId("vote-send").click();
  await expect(page.getByTestId("vote-result-title")).toContainText("sent");
  // The vote re-reads the list: the proposal no longer needs a vote, and the sheet stays.
  await expect(page.getByTestId(`proposal-row-${name}`)).toHaveCount(0);
  await expect(page.getByTestId("vote-result")).toBeVisible();
  await page.getByTestId("vote-done").click();
  await expect(page.getByTestId("vote-sheet")).toBeHidden();
});

test("a vote the engine refuses as a whole says why and keeps the form", async ({ page }) => {
  await open(page, "governance");
  await page.evaluate(() => window.__dwFixture!.fail("governance_vote", "governancePhase"));
  await page.getByTestId("proposal-row-dashcon-2027-venue").click();
  await page.getByTestId("proposal-vote").click();
  await page.getByTestId("vote-outcome-yes").click();
  await page.getByTestId("vote-passphrase").fill("demo");
  await page.getByTestId("vote-send").click();
  await expect(page.getByTestId("vote-error")).toHaveText("Proposals are still loading. Try again once they finish.");
  await expect(page.getByTestId("vote-result")).toHaveCount(0);
  await expect(page.getByTestId("vote-send")).toBeVisible();
});

test("the outcome is a radiogroup: one tab stop, the arrow keys choose (n1)", async ({ page }) => {
  await open(page, "governance");
  await page.getByTestId("proposal-row-dashcon-2027-venue").click();
  await page.getByTestId("proposal-vote").click();
  const group = page.getByRole("radiogroup", { name: "Your vote" });
  await expect(group.getByRole("radio")).toHaveCount(3);
  await page.getByTestId("vote-outcome-yes").click();
  await expect(page.getByTestId("vote-outcome-yes")).toHaveAttribute("aria-checked", "true");
  await page.keyboard.press("ArrowRight");
  await expect(page.getByTestId("vote-outcome-no")).toHaveAttribute("aria-checked", "true");
  await expect(page.getByTestId("vote-outcome-no")).toBeFocused();
  await expect(page.getByTestId("vote-outcome-yes")).toHaveAttribute("tabindex", "-1");
  await page.keyboard.press("ArrowLeft");
  await page.keyboard.press("ArrowLeft");
  await expect(page.getByTestId("vote-outcome-abstain")).toHaveAttribute("aria-checked", "true");
  expect(await seriousAxeViolations(page)).toEqual([]);
});

test("sync off: the Disabled notice turns governance on", async ({ page }) => {
  await open(page, "governance-disabled");
  await expect(page.getByTestId("governance-disabled")).toBeVisible();
  await page.getByTestId("governance-turn-on").click();
  await expect(page.getByTestId("proposal-list")).toBeVisible();
});

test("states: syncing, failed, no voting key, watch-only", async ({ page }) => {
  await open(page, "governance-syncing");
  await expect(page.getByTestId("governance-syncing")).toContainText("1,204 votes so far");
  await expect(page.getByTestId("governance-budget")).toContainText("What passing proposals take shows after sync");
  // Without the list, Mine still shows (D32).
  await expect(page.getByTestId("mine-row-dash-docs-refresh")).toBeVisible();
  await open(page, "governance-failed");
  await expect(page.getByTestId("governance-failed")).toBeVisible();
  await expect(page.getByTestId("mine-row-dash-docs-refresh")).toBeVisible();
  await open(page, "governance-peers-lost");
  // Synced once, then every peer dropped: the list and pane stay under the notice (D36).
  await expect(page.getByTestId("governance-failed")).toBeVisible();
  await expect(page.getByTestId("proposal-list")).toBeVisible();
  await expect(page.getByTestId("proposal-pane-chip")).toHaveText("Passing");
  await open(page, "governance-banned");
  await expect(page.getByTestId("proposal-pane-masternodes")).toContainText("PoSe banned");
  await expect(page.getByTestId("proposal-vote")).toBeDisabled();
  await expect(page.getByTestId("proposal-vote-blocked")).toHaveText("No masternode in this wallet can vote on this proposal");
  await open(page, "governance-empty");
  await expect(page.getByTestId("proposals-empty")).toContainText("No proposals this cycle");
  await open(page, "governance-no-masternodes");
  await expect(page.getByTestId("governance-browsing-only")).toBeVisible();
  await expect(page.getByTestId("proposal-vote")).toBeDisabled();
  await open(page, "governance-watch-only");
  await expect(page.getByTestId("governance-notice-watch-only")).toBeVisible();
  await expect(page.getByTestId("governance-create")).toBeDisabled();
  await expect(page.getByTestId("proposal-vote")).toBeDisabled();
});

test("Mine: Submit only where MG-06 D24 allows it, a failed resend said on the row", async ({ page }) => {
  await open(page, "governance-submit-fails");
  await page.getByTestId("governance-filter-mine").click();
  await expect(page.getByTestId("mine-list")).toBeVisible();
  for (const name of ["desktop-wallet-i18n", "community-meetups", "wallet-audit-2027"]) await expect(page.getByTestId(`mine-submit-${name}`)).toBeVisible();
  for (const name of ["dash-docs-refresh", "school-outreach", "merchant-kit", "dash-incubator-nov", "podcast-season-3", "old-hackathon"])
    await expect(page.getByTestId(`mine-submit-${name}`)).toHaveCount(0);
  await page.getByTestId("mine-submit-desktop-wallet-i18n").click();
  await expect(page.getByTestId("mine-chip-desktop-wallet-i18n")).toHaveText("Fee not sent");
  await expect(page.getByTestId("mine-error-desktop-wallet-i18n")).toBeVisible();
  // The badge counts the proposals a failure left needing the user (D5).
  await expect(page.getByTestId("sidebar-masternodes-badge")).toHaveText("3");
  expect(await seriousAxeViolations(page)).toEqual([]);
});

test("Mine: a slow Submit says Sending… and keeps the row (D17)", async ({ page }) => {
  await open(page, "governance-mine");
  await page.getByTestId("governance-filter-mine").click();
  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("governance_submit_proposal"));
  await page.getByTestId("mine-submit-desktop-wallet-i18n").click();
  await expect(page.getByTestId("mine-submit-desktop-wallet-i18n")).toHaveText("Sending…");
  await expect(page.getByTestId("mine-submit-desktop-wallet-i18n")).toBeDisabled();
  await expect(page.getByTestId("mine-row-desktop-wallet-i18n")).toBeVisible();
  await release.evaluate((r) => r());
  await expect(page.getByTestId("mine-submit-desktop-wallet-i18n")).toHaveText("Submit");
  await expect(page.getByTestId("mine-chip-desktop-wallet-i18n")).toHaveText("Fee paid");
});

test("the sidebar shows Masternodes only with the setting or a proposal in progress", async ({ page }) => {
  await page.goto(fixtureUrl("funded", "light"));
  await page.getByTestId("toolbar").waitFor();
  await expect(page.getByTestId("sidebar-masternodes")).toHaveCount(0);
  await command(page, { kind: "navigate", route: "settings" });
  await page.getByTestId("settings-show-masternodes").check();
  await expect(page.getByTestId("settings-governance-ask")).toBeVisible();
  await page.getByTestId("settings-governance-turn-on").click();
  await expect(page.getByTestId("sidebar-masternodes")).toBeVisible();
});

for (const theme of THEMES) {
  test(`create ${theme}: details, payments, review, then pay; Mine shows it`, async ({ page }) => {
    const errors = await open(page, "governance", theme);
    await page.getByTestId("governance-create").click();
    const sheet = page.getByTestId("create-sheet");
    await expect(sheet).toBeVisible();
    // Step 1 checks the name locally.
    await page.getByTestId("create-name").fill("Bad Name");
    await page.getByTestId("create-continue").click();
    await expect(page.getByTestId("create-error-name")).toBeVisible();
    await expect(page.getByTestId("create-error-url")).toBeVisible();
    await page.getByTestId("create-name").fill("desktop-translations");
    await page.getByTestId("create-url").fill("https://www.dashcentral.org/p/desktop-translations");
    await fontsReady(page);
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("create-continue").click();
    await expect(page.getByTestId("create-step-2")).toBeVisible();
    await page.getByTestId("create-count").selectOption("3");
    await page.getByTestId("create-amount").fill("12.5");
    await expect(page.getByTestId("create-total")).toContainText("37.5");
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("create-continue").click();
    await expect(page.getByTestId("create-review")).toBeVisible();
    await expect(page.getByTestId("create-review-payments")).toContainText("× 3");
    // The 1 DASH fee (burnt) and the network fee, apart and in all (D33).
    await expect(page.getByTestId("create-review-fee")).toHaveText(/1\.00000000\s*tDASH \+ network fee 0\.00000227\s*tDASH/);
    await expect(page.getByTestId("create-review-total")).toHaveText(/1\.00000227\s*tDASH/);
    await expect(page.getByTestId("create-fee-warning")).toBeVisible();
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("create-passphrase").fill("demo");
    await page.getByTestId("create-pay").click();
    await expect(sheet).toBeHidden();
    await expect(page.getByTestId("mine-list")).toBeVisible();
    await expect(page.getByTestId("mine-chip-desktop-translations")).toHaveText("Fee paid");
    await expect(page.getByTestId("mine-submit-desktop-translations")).toBeVisible();
    expect(errors).toEqual([]);
  });
}

test("create: an address the engine refuses goes back to Payments with the field's message", async ({ page }) => {
  await open(page, "governance");
  await page.getByTestId("governance-create").click();
  await page.getByTestId("create-name").fill("desktop-translations");
  await page.getByTestId("create-url").fill("https://www.dashcentral.org/p/desktop-translations");
  await page.getByTestId("create-continue").click();
  await page.getByTestId("create-amount").fill("10");
  await page.getByTestId("create-pay-to").selectOption("other");
  await page.getByTestId("create-address").fill("Xnotatestnetaddress");
  await page.getByTestId("create-continue").click();
  await expect(page.getByTestId("create-step-2")).toBeVisible();
  await expect(page.getByTestId("create-error-payment_address")).toHaveText("Enter a Dash address on testnet.");
  await expect(page.getByTestId("create-address")).toHaveAttribute("aria-invalid", "true");
});

/** Steps 1 and 2 of the create wizard with a valid draft, to the review. */
async function toReview(page: Page, name = "desktop-translations") {
  await page.getByTestId("governance-create").click();
  await page.getByTestId("create-name").fill(name);
  await page.getByTestId("create-url").fill(`https://www.dashcentral.org/p/${name}`);
  await page.getByTestId("create-continue").click();
  await page.getByTestId("create-amount").fill("10");
  await page.getByTestId("create-continue").click();
  await expect(page.getByTestId("create-review")).toBeVisible();
}

const createCalls = (page: Page) => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "governance_create_proposal").length);

test("create: a network fee that changed at Pay stops for a fresh confirm, then pays that one (D33)", async ({ page }) => {
  await open(page, "governance");
  await toReview(page);
  await expect(page.getByTestId("create-review-total")).toHaveText(/1\.00000227\s*tDASH/);
  // The coins moved after the review: Pay's re-read finds a higher network fee.
  await page.evaluate(() => (window.__dwFixture!.world.governance.networkFee = 4_500));
  await page.getByTestId("create-passphrase").fill("demo");
  await page.getByTestId("create-pay").click();
  await expect(page.getByTestId("create-fee-changed")).toContainText("The network fee changed to 0.00004500 tDASH, so you pay 1.00004500 tDASH in all. Nothing was paid");
  await expect(page.getByTestId("create-review-total")).toHaveText(/1\.00004500\s*tDASH/);
  expect(await createCalls(page)).toBe(0);
  // The next press confirms the figures now shown; the fixture refuses a grant above them plus the margin.
  await page.getByTestId("create-passphrase").fill("demo");
  await page.getByTestId("create-pay").click();
  await expect(page.getByTestId("create-sheet")).toBeHidden();
  await expect(page.getByTestId("mine-chip-desktop-translations")).toHaveText("Fee paid");
  expect(await createCalls(page)).toBe(1);
});

test("create: a background re-read never changes the figures the review shows; Pay still stops (D33)", async ({ page }) => {
  await open(page, "governance");
  await toReview(page);
  const previews = () => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "governance_preview_proposal").length);
  const before = await previews();
  // The coins move and an engine event re-reads every governance query, the preview included.
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.world.governance.networkFee = 4_500;
    f.emit({ kind: "governance", network: f.world.launch.network.dirName });
  });
  await expect.poll(previews).toBeGreaterThan(before);
  await expect(page.getByTestId("create-review-total")).toHaveText(/1\.00000227\s*tDASH/);
  await page.getByTestId("create-passphrase").fill("demo");
  await page.getByTestId("create-pay").click();
  await expect(page.getByTestId("create-fee-changed")).toContainText("The network fee changed to 0.00004500 tDASH");
  await expect(page.getByTestId("create-review-total")).toHaveText(/1\.00004500\s*tDASH/);
  expect(await createCalls(page)).toBe(0);
});

test("create: a first superblock that passes on the review takes a fresh preview and says so (D33)", async ({ page }) => {
  await open(page, "governance");
  await toReview(page);
  const json = page.getByTestId("create-json");
  const before = JSON.parse((await json.textContent())!).start_epoch as number;
  // The chosen superblock (the second, the first's cutoff being near: MG-06 D13) leaves the
  // upcoming list: the wizard moves the first payment on.
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.world.governance.superblocksPassed = 2;
    f.emit({ kind: "governance", network: f.world.launch.network.dirName });
  });
  await expect(page.getByTestId("create-first-moved")).toContainText("The first payment moved to Dec");
  await expect.poll(async () => JSON.parse((await json.textContent())!).start_epoch as number).toBeGreaterThan(before);
  await page.getByTestId("create-passphrase").fill("demo");
  await page.getByTestId("create-pay").click();
  await expect(page.getByTestId("create-sheet")).toBeHidden();
  expect(await createCalls(page)).toBe(1);
});

test("create: a call that fails after the fee went out never offers Pay again; Mine resumes it (D34)", async ({ page }) => {
  await open(page, "governance");
  await page.evaluate(() => (window.__dwFixture!.world.governance.createFailsAfterPaying = true));
  await toReview(page);
  await page.getByTestId("create-passphrase").fill("demo");
  await page.getByTestId("create-pay").click();
  await expect(page.getByTestId("create-already-made")).toContainText("desktop-translations was made, and its fee may have gone out. Don't pay again");
  await expect(page.getByTestId("create-pay")).toBeDisabled();
  await expect(page.getByTestId("create-back")).toBeDisabled();
  await page.getByTestId("create-passphrase").fill("demo");
  await expect(page.getByTestId("create-pay")).toBeDisabled();
  expect(await seriousAxeViolations(page)).toEqual([]);
  await page.getByTestId("create-show-mine").click();
  await expect(page.getByTestId("create-sheet")).toBeHidden();
  await expect(page.getByTestId("mine-chip-desktop-translations")).toHaveText("Fee paid");
  await expect(page.getByTestId("mine-submit-desktop-translations")).toBeVisible();
  expect(await createCalls(page)).toBe(1);
});

test("create: a failed call whose proposal Mine can't read keeps Pay off until Mine answers (D34)", async ({ page }) => {
  await open(page, "governance");
  await page.evaluate(() => (window.__dwFixture!.world.governance.createFailsAfterPaying = true));
  await toReview(page);
  await page.evaluate(() => window.__dwFixture!.fail("governance_my_proposals", "none"));
  await page.getByTestId("create-passphrase").fill("demo");
  await page.getByTestId("create-pay").click();
  await expect(page.getByTestId("create-unknown")).toBeVisible();
  await expect(page.getByTestId("create-pay")).toBeDisabled();
  await page.evaluate(() => window.__dwFixture!.fail("governance_my_proposals", null));
  await page.getByTestId("create-check-again").click();
  await expect(page.getByTestId("create-already-made")).toBeVisible();
  await expect(page.getByTestId("create-pay")).toBeDisabled();
});

test("create: a failed payment keeps the review open and says why", async ({ page }) => {
  await open(page, "governance");
  await page.evaluate(() => window.__dwFixture!.fail("governance_create_proposal", "duffs"));
  await page.getByTestId("governance-create").click();
  await page.getByTestId("create-name").fill("desktop-translations");
  await page.getByTestId("create-url").fill("https://www.dashcentral.org/p/desktop-translations");
  await page.getByTestId("create-continue").click();
  await page.getByTestId("create-amount").fill("10");
  await page.getByTestId("create-continue").click();
  await page.getByTestId("create-passphrase").fill("demo");
  await page.getByTestId("create-pay").click();
  await expect(page.getByTestId("create-error")).not.toBeEmpty();
  await expect(page.getByTestId("create-step-3")).toBeVisible();
  // Mine has no such proposal: nothing was paid, so Pay comes back (D34).
  await expect(page.getByTestId("create-pay")).toBeEnabled();
  await expect(page.getByTestId("create-already-made")).toHaveCount(0);
});

test("Governance numbers and the status bar's voting item", async ({ page }) => {
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl("governance", "light"));
  await page.getByTestId("toolbar").waitFor();
  const item = page.getByTestId("status-governance");
  await expect(item).toHaveText("Oct 10 votes: about 1 d 7 h left");
  await item.click();
  await expect(page.getByTestId("page-masternodes")).toBeVisible();
  await page.getByTestId("governance-more").click();
  await page.getByTestId("governance-more-numbers").click();
  await expect(page.getByTestId("numbers-cycle")).toContainText("Block 2,375,048 · about Oct 10");
  await expect(page.getByTestId("numbers-votes")).toContainText("2,850 masternodes and 180 evonodes");
  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
  await page.getByTestId("numbers-done").click();
  await expect(page.getByTestId("numbers-sheet")).toBeHidden();
  // No voting key in the wallet: no status item.
  await page.goto(fixtureUrl("governance-no-masternodes", "light"));
  await page.getByTestId("toolbar").waitFor();
  await expect(page.getByTestId("sidebar-masternodes")).toBeVisible();
  await expect(page.getByTestId("status-governance")).toHaveCount(0);
});
