import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, fontsReady, moveFromPlatform, THEMES, trackPageErrors, type Theme } from "./helpers";

// #773: Platform's structure pre-check refuses a move to the Dash wallet, a send to a Platform address
// and a top-up from Platform before anything is signed. Each says nothing was sent and the balance is
// unchanged, never the consensus error, and adds no Activity row. The exact retry of a held send and
// the move out of a held top-up say instead that this attempt sent nothing and the earlier item may
// still go through. `DW_SHOTS_DIR` also writes each state's screenshot for the pull request.

test.describe.configure({ timeout: 60_000 });

const PAYEE = "tdash1kq7r3m9x2v5c8n4z6w0e2t4y6u8p0s2a4d6f8h";
const RAW = /FeeStrategy|Fee strategy|credits\.|stopped with/;
const KINDS = [
  {
    kind: "refused",
    line: "Dash Platform would refuse this transfer as Dash Wallet built it, so nothing was sent and your balance is unchanged. If this keeps happening, update Dash Wallet.",
    retry:
      "Dash Platform would refuse this attempt as Dash Wallet built it, so this attempt sent nothing. Your earlier payment may still go through, so check Activity before you pay again. If this keeps happening, update Dash Wallet.",
    topUp:
      "Dash Platform would refuse this move as Dash Wallet built it, so this attempt sent nothing. Your earlier top-up may still go through, so check Activity before you try again. If this keeps happening, update Dash Wallet.",
  },
  {
    kind: "rulesUnsupported",
    line: "Dash Platform's rules have changed, and this version of Dash Wallet can't send this under them. Nothing was sent, and your balance is unchanged. Update Dash Wallet, then try again.",
    retry:
      "Dash Platform's rules have changed, and this version of Dash Wallet can't send this under them, so this attempt sent nothing. Your earlier payment may still go through. Update Dash Wallet, and check Activity before you pay again.",
    topUp:
      "Dash Platform's rules have changed, and this version of Dash Wallet can't make this move under them, so this attempt sent nothing. Your earlier top-up may still go through. Update Dash Wallet, and check Activity before you try again.",
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

/** Whether Activity still shows the earlier item `id` as able to go through. */
const stillHeld = (page: Page, id: string) =>
  page.evaluate((i) => window.__dwFixture!.world.pending.platform!.activity.some((a) => a.id === i && a.mayLand), id);

for (const theme of THEMES) {
  for (const { kind, line, retry, topUp } of KINDS) {
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

    test(`the retry of a held send the pre-check refuses: ${kind} (${theme})`, async ({ page }) => {
      const errors = await open(page, "platform-send-failed-may-land", theme);
      await page.getByTestId("sidebar-pay").click();
      await page.getByTestId("pay-address").fill(PAYEE);
      await page.getByTestId("platform-send-intro").waitFor();
      await page.getByTestId("pay-amount").fill("0.01");
      await page.getByTestId("pay-fee-line").waitFor();
      await page.getByTestId("pay-continue").click();
      const before = await platformState(page);
      await refuseNext(page, kind);
      await page.getByTestId("platform-send-go").click();
      await authorize(page);
      const refusal = page.getByTestId("platform-send-refusal");
      await expect(refusal).toHaveText(retry);
      await expect(refusal).not.toHaveText(RAW);
      expect(await platformState(page)).toBe(before);
      expect(await stillHeld(page, "ps-held")).toBe(true);
      await shot(page, theme, `send-retry-${kind}`);
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });

    test(`the move out of a held top-up the pre-check refuses: ${kind} (${theme})`, async ({ page }) => {
      const errors = await open(page, "dashpay-registered", theme);
      await page.evaluate(() => void (window.__dwFixture!.world.pending.platform!.nextTopUp = "held"));
      await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "profile" }));
      await page.getByTestId("profile-top-up").click();
      await page.getByTestId("topup-source-platform").click();
      await page.getByTestId("topup-next").click();
      await page.getByTestId("topup-pay").click();
      await authorize(page);
      await page.getByTestId("topup-move-to-core").click();
      await expect(page.getByTestId("transfer-sheet")).toBeVisible();
      await page.getByTestId("transfer-continue").click();
      await expect(page.getByTestId("platform-move-quote")).toBeVisible();
      const held = await page.evaluate(() => window.__dwFixture!.world.pending.platform!.activity.find((a) => a.kind === "toIdentity" && a.mayLand)!.id);
      const before = await platformState(page);
      await refuseNext(page, kind);
      await page.getByTestId("platform-move-go").click();
      await authorize(page);
      const refusal = page.getByTestId("platform-move-refusal");
      await expect(refusal).toHaveText(topUp);
      await expect(refusal).not.toHaveText(RAW);
      expect(await platformState(page)).toBe(before);
      expect(await stillHeld(page, held)).toBe(true);
      await shot(page, theme, `move-held-topup-${kind}`);
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }
}
