// "Sign in to an app" on the fixture backend, shared by app-connect.spec.ts and visual.spec.ts: a
// request delivered as dw-app's shell routes one, by the fixture request's outcome.

import type { Page } from "@playwright/test";
import { CONNECT_HOST, CONNECT_LINKS, type ConnectOutcome } from "../src/fixture/connect";
import { fixtureUrl, type Theme } from "./helpers";

export { CONNECT_HOST, CONNECT_LINKS };

/** The fixture app's id, as the sheet shows it. */
export const APP_ID = "a7c3e5f1".repeat(8);

/** Delivers the fixture request of `outcome`; one the OS delivered returns to CONNECT_HOST. */
export async function deliverConnect(page: Page, outcome: ConnectOutcome, returnsTo: string | null = CONNECT_HOST) {
  await page.evaluate((r) => window.__dwFixture!.deliverLinks([r]), { kind: "appConnect", linkId: CONNECT_LINKS[outcome], returnsTo } as const);
}

/** Opens `dashpay-registered` and the sign-in sheet of `outcome`, once it has read the request. */
export async function openConnect(page: Page, outcome: ConnectOutcome, theme: Theme = "light", returnsTo: string | null = CONNECT_HOST) {
  await page.goto(fixtureUrl("dashpay-registered", theme));
  await page.getByTestId("hero").waitFor();
  await deliverConnect(page, outcome, returnsTo);
  const sheet = page.getByTestId("connect-sheet");
  await sheet.waitFor();
  await page.locator("[data-testid=connect-body]:not([data-phase=loading])").waitFor();
  return sheet;
}

/** The sheet with its quote read: Approve is on. */
export async function quoted(page: Page) {
  await page.getByTestId("connect-fee").filter({ hasText: "in credits" }).waitFor();
}
