// U-06 fixture states shared by coinjoin.spec.ts and visual.spec.ts.

import type { Page } from "@playwright/test";

/** Each CoinJoin state (docs/design/U-06-coinjoin.md S2–S14), its scenario, title, and whether the
 * Home card shows it (D1). `start`: pressed Start on the page, back on Home (`pressStart`). */
export const STATES: Record<string, { scenario: string; extra?: string; start?: true; title: string; home: boolean }> = {
  readFailed: { scenario: "funded", extra: "&fail=coinjoin_status:none", title: "Could not read the mixing status", home: false },
  stopping: { scenario: "coinjoin-stopping", title: "Stopping…", home: true },
  starting: { scenario: "funded", start: true, title: "Starting…", home: true },
  paused: { scenario: "coinjoin-paused", title: "Paused", home: true },
  finished: { scenario: "coinjoin-finished", title: "Fully Mixed", home: true },
  mixing: { scenario: "coinjoin-mixing", title: "Mixing", home: true },
  disabled: { scenario: "coinjoin-disabled", title: "CoinJoin is turned off", home: false },
  watchOnly: { scenario: "coinjoin-watch-only", title: "Watch-only wallets cannot mix.", home: false },
  locked: { scenario: "coinjoin-locked", title: "Mixing stopped", home: true },
  error: { scenario: "coinjoin-error", title: "Mixing stopped", home: true },
  tooLittle: { scenario: "empty", title: "Not Started", home: false },
  done: { scenario: "coinjoin-custom", title: "Fully Mixed", home: false },
  off: { scenario: "funded", title: "Not Started", home: false },
};

/** Start Mixing from the page, then back to Home: Starting… is only a start this window saw, from
 * the balances it read stopped (D16). The fixture's mixer then waits for a test to move it. */
export async function pressStart(page: Page) {
  // The page's clock stands still (its timers run): Starting… outlasts D16's 20 s window.
  await page.clock.setFixedTime(new Date("2026-10-16T12:00:00Z"));
  const go = (route: "coinJoin" | "home") => page.evaluate((route) => window.__dwFixture!.shellCommand({ kind: "navigate", route }), route);
  await go("coinJoin");
  await page.locator("[data-testid=coinjoin-status][data-state=off]").waitFor();
  await page.getByTestId("coinjoin-action").click();
  await page.locator("[data-testid=coinjoin-status][data-state=starting]").waitFor();
  await go("home");
}
