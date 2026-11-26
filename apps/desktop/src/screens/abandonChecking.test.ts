// An abandoned payment while the engine's rescan checks a coin it spent before giving it back
// (Q100 E2): its details say so, and stop saying so once the engine reports the check done.

import { clearMocks } from "@tauri-apps/api/mocks";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import type { TxDetailExtrasInfo } from "../api/generated/TxDetailExtrasInfo";
import { installFixture, type FixtureHandle } from "../fixture/index";
import { ABANDON_CHECKING_TXID as TXID, FIXTURE_WALLET_ID as W } from "../fixture/scenarios";
import { en } from "../i18n/en";
import { CheckingSpentCoinsLine } from "./TxDetailSheet";

const TEXT = en.activity.abandon.checkingSpentCoins.replace(/'/g, "&#x27;");
const line = (x: Pick<TxDetailExtrasInfo, "abandoned" | "checkingSpentCoins"> | undefined) => renderToStaticMarkup(createElement(CheckingSpentCoinsLine, { x }));

let fixture: FixtureHandle;
beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  fixture = installFixture({ scenario: "abandon-checking" });
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

describe("the checking line of an abandoned payment", () => {
  it("shows while the engine checks a coin the abandoned payment spent", async () => {
    const x = await cmd.txDetailExtras(W, TXID);
    expect(x).toMatchObject({ abandoned: true, checkingSpentCoins: true });
    const html = line(x);
    expect(html).toContain('data-testid="tx-detail-checking-spent-coins"');
    expect(html).toContain(TEXT);
  });

  it("doesn't show once no coin is being checked", () => {
    expect(line({ abandoned: true, checkingSpentCoins: false })).toBe("");
  });

  it("doesn't show on a payment that isn't abandoned", () => {
    expect(line({ abandoned: false, checkingSpentCoins: true })).toBe("");
    expect(line(undefined)).toBe("");
  });

  it("clears when the details are read again after the check ends, with no restart", async () => {
    expect(line(await cmd.txDetailExtras(W, TXID))).toContain(TEXT);
    fixture.world.send.checkingSpentCoins = [];
    const after = await cmd.txDetailExtras(W, TXID);
    expect(after.checkingSpentCoins).toBe(false);
    expect(line(after)).toBe("");
  });

  it("isn't on another payment's details", async () => {
    const other = fixture.world.wallets[0].rows.find((r) => r.txid !== TXID && r.category === "sent")!;
    expect(line(await cmd.txDetailExtras(W, other.txid))).toBe("");
  });
});
