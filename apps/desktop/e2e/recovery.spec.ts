import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, trackPageErrors } from "./helpers";

// Unfinished transfers on Home (src/screens/UnfinishedTransfersCard.tsx) on the fixture backend
// (src/fixture/recovery.ts): the card for a restored Core → Shielded lock, and its Details sheet.

async function open(page: Page, scenario: string) {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, "light"));
  await page.getByTestId("hero").waitFor();
  return errors;
}

test.describe("Unfinished transfers", () => {
  test("recovery-one: the card offers Finish, and the card goes once it is applied", async ({ page }) => {
    const errors = await open(page, "recovery-one");
    const card = page.getByTestId("unfinished-card");
    await expect(card).toContainText("0.50 tDASH was locked for your Shielded balance and isn't added yet.");
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("unfinished-finish").click();
    await authorize(page);
    await expect(card).toHaveCount(0);
    expect(errors).toEqual([]);
  });

  test("recovery-restored-mixed: Details lists the three restored locks", async ({ page }) => {
    await open(page, "recovery-restored-mixed");
    await expect(page.getByTestId("unfinished-text")).toContainText("2 transfers from before the restore aren't finished");
    await page.getByTestId("unfinished-details").click();
    await expect(page.getByTestId("unfinished-row")).toHaveCount(3);
    await expect(page.getByTestId("unfinished-footer")).toHaveText("Nothing more is paid: finishing only adds money that's already locked.");
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("unfinished-row-finish").first().click();
    await authorize(page);
    await expect(page.getByTestId("unfinished-row-result").first()).toHaveText("Added to Shielded");
  });

  test("recovery-finish-fails: the failure shows with Retry, and the retry applies", async ({ page }) => {
    await open(page, "recovery-finish-fails");
    await page.getByTestId("unfinished-finish").click();
    await authorize(page);
    await expect(page.getByTestId("unfinished-text")).toContainText("Dash Platform did not answer");
    await page.getByTestId("unfinished-retry").click();
    await authorize(page);
    await expect(page.getByTestId("unfinished-card")).toHaveCount(0);
  });

  test("recovery-checking: Check again keeps the card while the lock stays unconfirmed", async ({ page }) => {
    await open(page, "recovery-checking");
    await expect(page.getByTestId("unfinished-text")).toContainText("1 transfer may already be finished");
    await page.getByTestId("unfinished-checkAgain").click();
    await authorize(page);
    await expect(page.getByTestId("unfinished-text")).toContainText("1 transfer may already be finished");
  });

  for (const scenario of ["funded", "recovery-confirming", "recovery-not-active"]) {
    test(`${scenario}: Home has no card`, async ({ page }) => {
      await open(page, scenario);
      await expect(page.getByTestId("shielded-card")).toBeVisible();
      await expect(page.getByTestId("unfinished-card")).toHaveCount(0);
    });
  }
});
