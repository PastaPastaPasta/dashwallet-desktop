import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";
import { callsOf, lastGrant } from "./transfer";

// Send from Shielded (Pay's link): Max to another Shielded address, and the sheet's sync line. The
// fixture's last pass ended 20 s before FIXTURE_NOW, so the clock is pinned to it.

const TO = "tdash1zq0k8yv6m2e3x9n4c5w7a8s0d2f4g6h8j0k2l4m6n8p0q2r4s6t8u0v2w4x6y8z0a2c4e6g8j0m2";

async function shot(page: Page, name: string) {
  const dir = process.env.DW_SHOTS;
  if (dir) {
    await fontsReady(page);
    await page.screenshot({ path: `${dir}/${name}.png` });
  }
}

async function openSend(page: Page, scenario: string, theme: Theme = "light") {
  const errors = trackPageErrors(page);
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("sidebar-pay").click();
  await page.getByTestId("shielded-send").click();
  await expect(page.getByTestId("shielded-sheet")).toBeVisible();
  return errors;
}

async function reasonOf(page: Page, testId: string) {
  const button = page.getByTestId(testId);
  await expect(button).toBeDisabled();
  const id = await button.getAttribute("aria-describedby");
  expect(id, `${testId} is described`).toBeTruthy();
  return (await page.locator(`[id="${id}"]`).textContent())!.trim();
}

test.describe("Send from Shielded: Max", () => {
  async function max(page: Page) {
    await page.getByTestId("shielded-to").fill(TO);
    await page.getByTestId("shielded-max").click();
    // 1.25 less the fixture's fee of 0.0002: the whole quote, then one re-quote of what it leaves.
    await expect(page.getByTestId("shielded-amount")).toHaveValue("1.24980000");
    expect(await callsOf(page, "shielded_send_quote")).toBe(2);
    await expect(page.getByTestId("shielded-amount-problem")).toHaveText("");
    await expect(page.getByTestId("shielded-next")).toBeEnabled();
  }

  for (const theme of THEMES) {
    test(`fills the balance less the fee once an address is typed (${theme})`, async ({ page }) => {
      const errors = await openSend(page, "funded", theme);
      expect(await reasonOf(page, "shielded-max")).toBe("Enter a Shielded address first.");
      await shot(page, `${theme}-send-empty`);
      await page.getByTestId("shielded-to").fill(TO);
      await expect(page.getByTestId("shielded-max")).toBeEnabled();
      await shot(page, `${theme}-send-form`);
      await max(page);
      await shot(page, `${theme}-send-max`);
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });

    test(`Confirm quotes Max's amount again and sends no more than that quote (${theme})`, async ({ page }) => {
      const errors = await openSend(page, "funded", theme);
      await max(page);
      await page.getByTestId("shielded-next").click();
      await expect(page.getByTestId("shielded-quote-total")).toContainText("1.25");
      expect(await callsOf(page, "shielded_send_quote")).toBe(3);
      await shot(page, `${theme}-send-max-confirm`);
      if (theme === "dark") return;
      await page.getByTestId("shielded-go").click();
      await authorize(page);
      await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Sent");
      expect(await lastGrant(page)).toEqual({ kind: "platformOp", maxDuffs: 0, maxCredits: "125000000000" });
      await expect.poll(() => page.evaluate(() => window.__dwFixture!.world.pending.shielded!.balance)).toBe("0");
      expect(errors).toEqual([]);
    });
  }

  test("says why when the balance does not cover the fee, and fills nothing", async ({ page }) => {
    await openSend(page, "shielded-below-fee");
    await page.getByTestId("shielded-to").fill(TO);
    await page.getByTestId("shielded-max").click();
    await expect(page.getByTestId("shielded-amount-problem")).toHaveText(/^Not enough in Shielded: .* needed with the fee, .* available\.$/);
    await expect(page.getByTestId("shielded-amount")).toHaveValue("");
    await expect(page.getByTestId("shielded-amount")).toHaveAttribute("aria-invalid", "true");
    await shot(page, "light-send-max-below-fee");
    // Typing clears the refusal.
    await page.getByTestId("shielded-amount").fill("0.0001");
    await expect(page.getByTestId("shielded-amount-problem")).toHaveText("");
  });

  test("an address the engine refuses fills nothing and says so", async ({ page }) => {
    await openSend(page, "funded");
    await page.getByTestId("shielded-to").fill("XnotShielded");
    await page.getByTestId("shielded-max").click();
    await expect(page.getByTestId("shielded-amount-problem")).toHaveText("Enter a Shielded address of this network.");
    await expect(page.getByTestId("shielded-amount")).toHaveValue("");
  });

  test("discreet mode masks Max's fill", async ({ page }) => {
    await page.clock.setFixedTime(FIXTURE_NOW);
    await page.goto(fixtureUrl("funded", "light"));
    await page.getByTestId("toolbar-discreet").click();
    await page.getByTestId("sidebar-pay").click();
    await page.getByTestId("shielded-send").click();
    await page.getByTestId("shielded-to").fill(TO);
    await page.getByTestId("shielded-max").click();
    await expect(page.getByTestId("shielded-next")).toBeEnabled();
    await expect(page.getByTestId("shielded-amount")).toHaveValue(/^[#.,]+$/);
  });
});

test.describe("Send from Shielded: sync line", () => {
  for (const theme of THEMES) {
    test(`says Scanning… while a pass runs (${theme})`, async ({ page }) => {
      const errors = await openSend(page, "shielded-scanning", theme);
      await expect(page.getByTestId("shielded-sync")).toHaveText("Scanning…");
      await shot(page, `${theme}-send-scanning`);
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });

    test(`says when the last pass ended (${theme})`, async ({ page }) => {
      await openSend(page, "shielded-synced-earlier", theme);
      await expect(page.getByTestId("shielded-sync")).toHaveText("Last synced 3 hours ago");
      await shot(page, `${theme}-send-synced-earlier`);
    });
  }

  test("a pass that just ended reads just now", async ({ page }) => {
    await openSend(page, "funded");
    await expect(page.getByTestId("shielded-sync")).toHaveText("Last synced just now");
  });
});
