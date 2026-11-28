// pay-contact.ts's `run` against a stand-in driver (DP3-06): a rerun whose payment is already in
// the ledger still picks the payee and checks its card, and types and sends nothing.

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type { WebDriver } from "./driver.ts";
import type { Ledger, run as Run } from "./pay-contact.ts";

const USERNAME = "dwo4ab8372";
const TXID = "68f512a7608cf7a6c902a7ad0bbd773e61cdf6834839e0abe553e66d2963526e";
const PAID: Ledger = { payer: "A", payee: "B", payerIdentity: "idA", payeeIdentity: "idB", duffs: 100000, state: "sent", txid: TXID, at: "2026-11-22T18:00:00Z" };

/** A stand-in app: answers the flow's DOM reads, records what it presses, types and captures. */
function app(card: { name: string; sub: string | null }) {
  const did: string[] = [];
  const answer = (js: string): unknown => {
    if (js.includes("not a masked field")) return null;
    if (js.includes("status-bar")) return "Synced";
    if (js.includes("__dwFixture")) return false;
    if (js.includes("querySelectorAll")) return [`QA B @${USERNAME}`];
    if (js.includes(`includes('${USERNAME}')`)) return js.includes("pay-frequent-chip");
    if (js.includes("pay-payee-name")) return card;
    if (js.includes("pay-banner")) return null;
    if (js.includes("tx-detail-contact")) return `QA B @${USERNAME}`;
    return "";
  };
  const wd = {
    press: async (css: string) => void did.push(`press ${css}`),
    fill: async (css: string) => void did.push(`fill ${css}`),
    waitFor: async (css: string) => css,
    type: async (el: string) => void did.push(`fill ${el}`),
    click: async (el: string) => void did.push(`click ${el}`),
    waitGone: async () => undefined,
    find: async (css: string) => (css.includes(TXID) ? "row" : null),
    script: async (js: string) => answer(js),
    screenshot: async (name: string) => void did.push(`screenshot ${name}`),
  };
  return { wd: wd as unknown as WebDriver, did };
}

describe("pay-contact on a fixture already paid (DP3-06)", () => {
  let run: typeof Run;
  let dir: string;
  const saved = { ...process.env };
  beforeAll(async () => {
    dir = mkdtempSync(join(tmpdir(), "dw-pay-contact-"));
    Object.assign(process.env, {
      DW_APP: "/bin/true",
      DWCLI: "/bin/true",
      DW_TESTNET_FIXTURE: dir,
      DW_PAY_USERNAME: USERNAME,
      DW_PAY_DISPLAY_NAME: "QA B",
      DW_E2E_OUT: join(dir, "out"),
      DW_E2E_TIMEOUT_MS: "400",
      DW_E2E_DASHPAY_TIMEOUT_MS: "400",
      DW_E2E_SEND_TIMEOUT_MS: "400",
    });
    ({ run } = await import("./pay-contact.ts"));
  });
  afterAll(() => {
    process.env = { ...saved };
    rmSync(dir, { recursive: true, force: true });
  });

  const rerun = (wd: WebDriver) =>
    run("p", "idB", wd, PAID, () => {
      throw new Error("the ledger must not be written");
    }, { payer: "A", payee: "B", payerIdentity: "idA", payeeIdentity: "idB", duffs: 100000 }, () => undefined);

  it("checks the payee card and pays nothing", async () => {
    const { wd, did } = app({ name: "QA B", sub: `@${USERNAME}` });
    const r = await rerun(wd);
    expect(r.ledger).toBe(PAID);
    expect(did).toContain(`press [data-testid="pay-contacts"]`);
    expect(did.indexOf("screenshot 2-payee")).toBeGreaterThan(did.indexOf(`press [data-testid="pay-frequent-chip"][data-contact="idB"]`));
    expect(did.filter((d) => d.startsWith("fill")).map((d) => d.split(" ")[1])).toEqual([`[data-testid="lock-field"]`]);
    expect(did.some((d) => /pay-continue|pay-confirm-send|pay-amount/.test(d))).toBe(false);
  });

  it("fails when the payee card is wrong, though the payment was sent before", async () => {
    const { wd, did } = app({ name: `@${USERNAME}`, sub: "QA B" });
    await expect(rerun(wd)).rejects.toThrow(/the payee card shows name "@dwo4ab8372", not "QA B"/);
    expect(did.some((d) => /pay-continue|pay-confirm-send/.test(d))).toBe(false);
  });
});
