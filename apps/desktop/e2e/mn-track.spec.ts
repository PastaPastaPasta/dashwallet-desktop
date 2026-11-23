import { expect, test, type Page } from "@playwright/test";
import { CLUB, FRIEND, KEY_OTHER, KEY_VOTING } from "../src/fixture/masternodes";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors } from "./helpers";

// MG-10 S1 tracking on the fixture: Track a masternode, Add keys (a voting key too: MG-00 D13
// amended for 1.0), Keys you added with Remove, Stop tracking, and a tracked masternode voting on
// a proposal with the voting key added to it.

async function open(page: Page, scenario: string, theme: "light" | "dark" = "light") {
  const errors = trackPageErrors(page);
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  await page.getByTestId("sidebar-masternodes").click();
  await expect(page.getByTestId("page-masternodes")).toBeVisible();
  return errors;
}

const pane = (page: Page) => page.getByTestId("mn-pane");

async function addKey(page: Page, role: string, key: string) {
  const sheet = page.getByTestId("mn-add-key-sheet");
  await sheet.getByTestId(`mn-add-key-role-${role}`).click();
  await sheet.getByTestId("mn-add-key-input").fill(key);
  await sheet.getByTestId("mn-add-key-submit").click();
}

for (const theme of THEMES) {
  test(`add a voting key to a tracked masternode ${theme}: checked, listed, then it can vote; 0 serious axe issues`, async ({ page }) => {
    const errors = await open(page, "mn-track", theme);
    await expect(page.getByTestId(`mn-row-${CLUB}`)).toHaveAttribute("aria-current", "true");
    await expect(page.getByTestId("mn-vote")).toBeDisabled();
    await expect(pane(page)).toContainText("Its voting key is not in this wallet");
    await page.getByTestId("mn-more").click();
    await expect(page.getByRole("menuitem")).toHaveText(["Rename…", "Copy ProTx hash", "Service update or revoke…", "Add keys…", "Stop tracking"]);
    await page.getByTestId("mn-more-add-keys").click();
    const sheet = page.getByTestId("mn-add-key-sheet");
    await expect(sheet.getByTestId("mn-add-key-roles")).toContainText("Voting key");
    await expect(sheet.getByTestId("mn-add-key-roles")).toContainText("Payout key");
    await fontsReady(page);
    expect(await seriousAxeViolations(page)).toEqual([]);

    await addKey(page, "voting", KEY_OTHER);
    await expect(sheet.getByTestId("mn-add-key-error")).toHaveText("This key doesn't belong to this masternode.");
    await expect(sheet.getByTestId("mn-add-key-input")).toHaveValue("");
    await expect(sheet).not.toContainText(KEY_OTHER);
    await addKey(page, "voting", KEY_VOTING);
    await expect(sheet.getByTestId("mn-add-key-result")).toHaveText("Matches this masternode's voting key.");
    await sheet.getByTestId("mn-add-key-done").click();
    await expect(sheet).toHaveCount(0);

    await expect(page.getByTestId("mn-key-added-voting")).toContainText("Voting key · matches");
    await expect(page.getByTestId("mn-vote")).toBeEnabled();
    await fontsReady(page);
    expect(await seriousAxeViolations(page)).toEqual([]);
    expect(errors).toEqual([]);
  });

  test(`a tracked masternode votes with its added key ${theme}`, async ({ page }) => {
    const errors = await open(page, "governance-added-key", theme);
    await page.getByTestId("proposal-vote").click();
    const sheet = page.getByTestId("vote-sheet");
    await expect(sheet.getByTestId("vote-node-c1c1c1c1")).toContainText("key you added");
    await expect(sheet.getByTestId("vote-node-b2b2b2b2")).toContainText("key in this wallet · voting key 2");
    await fontsReady(page);
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("vote-outcome-yes").click();
    await page.getByTestId("vote-passphrase").fill("demo");
    await page.getByTestId("vote-send").click();
    await expect(page.getByTestId("vote-result-c1c1c1c1")).toBeVisible();
    await expect(page.getByTestId("vote-result-c1c1c1c1")).not.toContainText("voting key is not in this wallet");
    expect(errors).toEqual([]);
  });
}

test("remove an added voting key: it leaves Keys you added and Vote turns off", async ({ page }) => {
  const errors = await open(page, "mn-track-voting");
  await expect(page.getByTestId("mn-vote")).toBeEnabled();
  await page.getByTestId("mn-key-remove-voting").click();
  await expect(page.getByTestId("mn-keys-added")).toHaveCount(0);
  await expect(page.getByTestId("mn-vote")).toBeDisabled();
  expect(errors).toEqual([]);
});

test("locked: Keys you added asks to unlock, and adding a key says so", async ({ page }) => {
  const errors = await open(page, "mn-track-voting-locked");
  await expect(page.getByTestId("mn-keys-added-locked")).toContainText("Keys you added: unlock to see");
  await page.getByTestId("mn-more").click();
  await page.getByTestId("mn-more-add-keys").click();
  await addKey(page, "voting", KEY_VOTING);
  await expect(page.getByTestId("mn-add-key-error")).toHaveText("Unlock to add a key.");
  await page.getByTestId("mn-add-key-unlock").click();
  await expect(page.getByTestId("mn-add-key-sheet")).toHaveCount(0);
  await page.getByTestId("unlock-sheet-field").fill("demo");
  await page.getByTestId("unlock-sheet-submit").click();
  await expect(page.getByTestId("unlock-sheet")).toHaveCount(0);
  await expect(page.getByTestId("mn-key-added-voting")).toContainText("Voting key · matches");
  await page.getByTestId("mn-more").click();
  await page.getByTestId("mn-more-add-keys").click();
  await addKey(page, "voting", KEY_VOTING);
  await expect(page.getByTestId("mn-add-key-result")).toHaveText("Matches this masternode's voting key.");
  expect(errors).toEqual([]);
});

test("track by IP → add keys opens → stop tracking", async ({ page }) => {
  const errors = await open(page, "mn-track");
  await page.getByTestId("mn-page-more").click();
  await page.getByTestId("mn-page-more-track").click();
  const sheet = page.getByTestId("mn-track-sheet");
  await sheet.getByTestId("mn-track-query").fill("not an address");
  await sheet.getByTestId("mn-track-find").click();
  await expect(sheet.getByTestId("mn-track-error")).toHaveText("Enter an IP address and port, or a 64-character ProTx hash.");
  await sheet.getByTestId("mn-track-query").fill("198.51.100.21");
  await sheet.getByTestId("mn-track-find").click();
  await expect(sheet.getByTestId(`mn-track-result-${CLUB}`)).toContainText("Already tracked");
  await expect(sheet.getByTestId("mn-track-submit")).toBeDisabled();
  await sheet.getByTestId("mn-track-query").fill("198.51.100.42");
  await sheet.getByTestId("mn-track-find").click();
  await expect(sheet.getByTestId(`mn-track-result-${FRIEND}`)).toContainText("Evonode · 198.51.100.42:19999");
  await sheet.getByTestId("mn-track-label").fill("Friend's evonode");
  await sheet.getByTestId("mn-track-submit").click();
  await expect(sheet).toHaveCount(0);
  // Add keys follows Track (MG-10 §1.3 step 3).
  const keys = page.getByTestId("mn-add-key-sheet");
  await expect(keys).toContainText("Friend's evonode");
  await keys.getByRole("button", { name: "Cancel" }).click();
  await expect(page.getByTestId("mn-pane-title")).toHaveText("Friend's evonode");
  await expect(page.getByTestId("mn-group-tracked")).toContainText("Friend's evonode");

  await page.getByTestId("mn-more").click();
  await page.getByTestId("mn-more-untrack").click();
  await expect(page.getByTestId("mn-untrack-sheet")).toContainText("Friend's evonode leaves My masternodes.");
  await page.getByTestId("mn-untrack-submit").click();
  await expect(page.getByTestId("mn-untrack-sheet")).toHaveCount(0);
  await expect(page.getByTestId(`mn-row-${FRIEND}`)).toHaveCount(0);
  expect(errors).toEqual([]);
});
