// DP1-08 fixture states shared by dashpay.spec.ts and visual.spec.ts.

import { expect, type Page } from "@playwright/test";
import { authorize } from "./helpers";

/** Each Home card state (docs/design/DP1-08-status-card.md S2–S17, DP1-10-my-profile.md S19–S20),
 * its scenario, title and chip. */
export const CARDS: Record<string, { scenario: string; extra?: string; title: string; chip: string | null }> = {
  error: { scenario: "dashpay-no-identity", extra: "&fail=dashpay_status:none", title: "Could not read DashPay's state", chip: null },
  looking: { scenario: "dashpay-starting", title: "Looking for your DashPay identity…", chip: null },
  notStarted: { scenario: "dashpay-not-started", title: "DashPay has not started", chip: null },
  unlockToLook: { scenario: "dashpay-identity-unsettled", title: "Unlock to look for your DashPay identity", chip: null },
  lookFailed: { scenario: "dashpay-look-failed", title: "Could not finish looking for your DashPay identity", chip: null },
  // No chip: Home's card and the sidebar are the ways in.
  join: { scenario: "dashpay-no-identity", title: "Get a DashPay username", chip: null },
  joinWaiting: { scenario: "dashpay-waiting-for-sync", title: "Get a DashPay username", chip: null },
  watchOnly: { scenario: "dashpay-watch-only", title: "DashPay needs this wallet's keys", chip: null },
  registering: { scenario: "dashpay-registering-unread", title: "Creating your username", chip: "Registering…" },
  creating: { scenario: "dashpay-registering", title: "Creating – @alice", chip: "@alice · Registering" },
  creatingUnlock: { scenario: "dashpay-registering-unlock", title: "Creating – @alice", chip: "@alice · Registering" },
  creatingPaused: { scenario: "dashpay-registering-paused", title: "Creating – @alice", chip: "@alice · Registering" },
  creatingHeld: { scenario: "dashpay-registering-held", title: "Creating – @alice", chip: "@alice · Registering" },
  creatingRepair: { scenario: "tools-stuck-funding", title: "Creating – @alice", chip: "@alice · Registering" },
  voting: { scenario: "dashpay-contest-pending", title: "Voting – @alice", chip: "@alice · Pending" },
  votingNoEnd: { scenario: "dashpay-contest-pending-no-end", title: "Voting – @alice", chip: "@alice · Pending" },
  noUsername: { scenario: "dashpay-no-username", title: "Finish username registration", chip: "No username" },
  // DP1-10 S19, S20.
  lowCredits: { scenario: "dashpay-credits-low", title: "Your credit balance is low", chip: "@alice" },
  emptyCredits: { scenario: "dashpay-low-credits", title: "Your credit balance is fully depleted", chip: "@alice" },
};

/** Each identity chip (DASHPAY §4, DP1-08 S9–S18) and a scenario that shows it. */
export const CHIPS: Record<string, string> = {
  registering: "dashpay-registering-unread",
  registeringLabel: "dashpay-registering",
  contest: "dashpay-contest-pending",
  identity: "dashpay-registered",
  noUsername: "dashpay-no-username",
  updating: "dashpay-names-updating",
  namesUnknown: "dashpay-names-unknown",
};

/** The text a chip settles on once the identities are read; until then it says "Updating…". */
export const CHIP_TEXT: Record<string, string> = {
  registeringLabel: "@alice",
  identity: "@alice",
  noUsername: "No username",
  namesUnknown: "Username not loaded",
};

/** On "Make your username private": the Dash wallet, then the name step. With Shielded able to pay
 * that is the Dash wallet row and Continue; while it cannot, "Continue without privacy". */
export async function payFromDashWallet(page: Page) {
  const step = page.getByTestId("registration-pay-with");
  await expect(step).toHaveAttribute("data-kind", /^(sources|tip)$/);
  if ((await step.getAttribute("data-kind")) === "tip") await page.getByTestId("privacy-without").click();
  else {
    await page.getByTestId("pay-with-core").getByRole("radio").check();
    await page.getByTestId("registration-next").click();
  }
  await page.getByTestId("registration-name-field").waitFor();
}

/** On "Make your username private": `source` behind Advanced when it is Platform, then Continue. */
export async function payFrom(page: Page, source: "shielded" | "platform") {
  await expect(page.getByTestId("registration-pay-with")).toHaveAttribute("data-kind", /^(sources|tip)$/);
  if (source === "platform") await page.getByTestId("privacy-advanced").click();
  await page.getByTestId(`pay-with-${source}`).getByRole("radio").check();
  await page.getByTestId("registration-next").click();
  await page.getByTestId("registration-name-field").waitFor();
}

/** From the Join page: the privacy step with the Shielded world changed by `world`, showing the
 * tip card `kind` (C-funding F16). */
export async function privacyTip(page: Page, world: Record<string, unknown>, kind: string) {
  await page.evaluate((w) => {
    const f = window.__dwFixture!;
    f.world.pending.shielded = { ...f.world.pending.shielded!, ...w };
    f.emit({ kind: "eventsOverflowed" } as never);
  }, world);
  await page.getByTestId("join-create").click();
  await expect(page.getByTestId("privacy-tip")).toHaveAttribute("data-kind", kind);
  await page.mouse.move(0, 0);
}

/** From the Join page: a registration paid from the Platform balance whose run fails, up to its
 * failed screen: its create fails in a block (C-funding F10), or its plan changed (F22). */
export async function platformRunFails(page: Page, label: string, outcome: "paidFailure" | "replanRefused") {
  await page.evaluate((o) => (window.__dwFixture!.world.dashpay.platformCreates = [o]), outcome);
  await page.getByTestId("join-create").click();
  await payFrom(page, "platform");
  await page.getByTestId("registration-name-field").fill(label);
  await expect(page.getByTestId("registration-next")).toBeEnabled();
  await page.getByTestId("registration-next").click();
  await page.getByTestId("registration-fee").waitFor();
  await page.getByTestId("registration-pay").click();
  await authorize(page);
  await page.evaluate(() => {
    const d = window.__dwFixture!.world.dashpay.registrations.at(-1)!.draft;
    while (window.__dwFixture!.advanceRegistration(d));
  });
  await expect(page.getByTestId("registration-screen")).toHaveAttribute("data-kind", "failed");
  // No step under the pointer.
  await page.mouse.move(0, 0);
}
