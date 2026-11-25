import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, fontsReady, moveFromPlatform, THEMES, trackPageErrors, type Theme } from "./helpers";

// #773: Platform's structure pre-check refuses a move to the Dash wallet, a send to a Platform address
// and a top-up from Platform before anything is signed. Each says nothing was sent and the balance is
// unchanged, never the consensus error, and adds no Activity row. `DW_SHOTS_DIR` also writes each
// state's screenshot for the pull request.

test.describe.configure({ timeout: 60_000 });

const PAYEE = "tdash1kq7r3m9x2v5c8n4z6w0e2t4y6u8p0s2a4d6f8h";
const RAW = /FeeStrategy|Fee strategy|credits\.|stopped with/;
const KINDS = [
  {
    kind: "refused",
    line: "Dash Platform would refuse this transfer as Dash Wallet built it, so nothing was sent and your balance is unchanged. If this keeps happening, update Dash Wallet.",
  },
  {
    kind: "rulesUnsupported",
    line: "Dash Platform's rules have changed, and this version of Dash Wallet can't send this under them. Nothing was sent, and your balance is unchanged. Update Dash Wallet, then try again.",
  },
] as const;
type Kind = (typeof KINDS)[number]["kind"];

const shots = process.env.DW_SHOTS_DIR;

async function shot(page: Page, theme: Theme, state: string) {
  if (!shots) return;
  await fontsReady(page);
  await page.screenshot({ path: `${shots}/${process.env.DW_SHOTS_TAG ?? "after"}-${theme}-${state}.png` });
}

async function open(page: Page, scenario: string, theme: Theme) {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  return errors;
}

/** The pre-check refuses the next move, send or top-up from the Platform addresses. */
const refuseNext = (page: Page, kind: Kind) => page.evaluate((k) => void (window.__dwFixture!.world.pending.platform!.preCheck = k), kind);

/** The Platform addresses' Activity rows and balances, which a refusal leaves as they were. */
const platformState = (page: Page) =>
  page.evaluate(() => {
    const p = window.__dwFixture!.world.pending.platform!;
    return JSON.stringify({ rows: p.activity.map((a) => a.id), addresses: p.addresses });
  });

for (const theme of THEMES) {
  for (const { kind, line } of KINDS) {
    test(`a move to the Dash wallet the pre-check refuses: ${kind} (${theme})`, async ({ page }) => {
      const errors = await open(page, "funded", theme);
      await moveFromPlatform(page);
      const before = await platformState(page);
      await refuseNext(page, kind);
      await page.getByTestId("platform-move-go").click();
      await authorize(page);
      const refusal = page.getByTestId("platform-move-refusal");
      await expect(refusal).toHaveText(line);
      await expect(refusal).not.toHaveText(RAW);
      await expect(page.getByTestId("platform-move-refused-back")).toHaveCount(0);
      expect(await platformState(page)).toBe(before);
      await shot(page, theme, `move-${kind}`);
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });

    test(`a send to a Platform address the pre-check refuses: ${kind} (${theme})`, async ({ page }) => {
      const errors = await open(page, "funded", theme);
      await page.getByTestId("sidebar-pay").click();
      await page.getByTestId("pay-tabs-address").click();
      await page.getByTestId("pay-address").fill(PAYEE);
      await page.getByTestId("platform-send-intro").waitFor();
      await page.getByTestId("pay-amount").fill("0.01");
      await page.getByTestId("pay-continue").click();
      const before = await platformState(page);
      await refuseNext(page, kind);
      await page.getByTestId("platform-send-go").click();
      await authorize(page);
      const refusal = page.getByTestId("platform-send-refusal");
      await expect(refusal).toHaveText(line);
      await expect(refusal).not.toHaveText(RAW);
      expect(await platformState(page)).toBe(before);
      await shot(page, theme, `send-${kind}`);
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });

    test(`a top-up from Platform the pre-check refuses: ${kind} (${theme})`, async ({ page }) => {
      const errors = await open(page, "dashpay-registered", theme);
      await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "profile" }));
      await page.getByTestId("profile-top-up").click();
      await page.getByTestId("topup-source-platform").click();
      await page.getByTestId("topup-preset-5000000").click();
      await page.getByTestId("topup-next").click();
      await expect(page.getByTestId("topup-quote")).toBeVisible();
      const before = await platformState(page);
      await refuseNext(page, kind);
      await page.getByTestId("topup-pay").click();
      await authorize(page);
      const refusal = page.getByTestId("topup-refusal");
      await expect(refusal).toHaveText(line);
      await expect(refusal).not.toHaveText(RAW);
      expect(await platformState(page)).toBe(before);
      await shot(page, theme, `topup-${kind}`);
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }
}
