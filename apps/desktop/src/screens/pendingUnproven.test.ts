// A coin the engine withholds while the payment that sent it may never confirm (Q100 E1): it isn't
// counted in any balance the app shows (condition (a), one test per surface), the coin row says why
// it can't be picked, and the payment's details show the pending line and Abandon's hint
// (condition (e)).

import { clearMocks } from "@tauri-apps/api/mocks";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import type { Balances } from "../api/generated/Balances";
import type { Utxo } from "../api/generated/Utxo";
import type { TxDetailExtrasInfo } from "../api/generated/TxDetailExtrasInfo";
import { CoinTable } from "../components/coins";
import { installFixture, type FixtureHandle } from "../fixture/index";
import { FIXTURE_WALLET_ID as W, PENDING_UNPROVEN_CHANGE as CHANGE, PENDING_UNPROVEN_TXID as TXID } from "../fixture/scenarios";
import { coreMaxPlan } from "../fixture/shielded";
import { display } from "../format/display";
import { coreSubtitle, homeBalance } from "../format/homeBalance";
import { en } from "../i18n/en";
import { notSelectable } from "./pay/model";
import { PendingUnprovenLine, UnprovenAbandonHint, withholds } from "./TxDetailSheet";

const FEE = { kind: "perKb", duffsPerKb: 1_000 } as const;
const WITHHELD = { txid: TXID, vout: 1 };
const isWithheld = (o: { txid: string; vout: number }) => o.txid === WITHHELD.txid && o.vout === WITHHELD.vout;
const PENDING_TEXT = en.coins.pendingUnproven;
const HINT_TEXT = en.activity.abandon.unprovenParentHint;

let fixture: FixtureHandle;
beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  fixture = installFixture({ scenario: "pending-unproven" });
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

async function coins(): Promise<Utxo[]> {
  return cmd.utxos(W, { includeLocked: true, fullyMixedOnly: false, minConfirmations: null });
}

/** What the wallet can spend besides the withheld coin, as the engine's selection counts it. */
async function spendableOthers(): Promise<number> {
  return (await coins()).filter((u) => u.spendable && !u.reserved && !u.userLocked && u.coinjoinRounds === null).reduce((n, u) => n + u.amount, 0);
}

describe("a withheld coin is in no balance the app shows (condition (a))", () => {
  it("lists the coin as withheld and not spendable, its amount reported apart", async () => {
    const withheld = (await coins()).find((u) => isWithheld(u.outpoint))!;
    expect(withheld).toMatchObject({ amount: CHANGE, spendable: false, excluded: "pendingUnproven" });
    const b = (await cmd.balance(W))!;
    expect(b.withheld).toBe(CHANGE);
  });

  it("Home: the hero and the Core row leave it out, and the Core row's subtitle shows it pending", async () => {
    const b = (await cmd.balance(W))!;
    const home = homeBalance(b, "absent", "absent");
    expect(home.total).toBe(b.confirmed + b.unconfirmed);
    expect(home.pools.core).toBe(b.total - b.immature - b.locked);
    expect(b.total).toBe(b.confirmed + b.unconfirmed + b.immature + b.locked);
    const fmt = (duffs: number) => String(duffs);
    expect(coreSubtitle(b, fmt, "Dash wallet")).toBe(`${b.unconfirmed + CHANGE} pending`);
    const none: Balances = { ...b, unconfirmed: 0, withheld: CHANGE };
    expect(coreSubtitle(none, fmt, "Dash wallet")).toBe(`${CHANGE} pending`);
  });

  it("Send's available amount leaves it out", async () => {
    const available = await cmd.maxSpendable(W, { kind: "any" }, FEE);
    expect(available).toBe(await spendableOthers());
    expect(available).toBeLessThan((await cmd.balance(W))!.confirmed + CHANGE);
  });

  it("Send's Max spends every other coin and not it", async () => {
    const available = await cmd.maxSpendable(W, { kind: "any" }, FEE);
    const d = await cmd.newTxDraft(W);
    await cmd.draftSetRecipients(d, [{ address: "yXdjeVZqmWp4ZcVJ2Hh8hcFhUzFE8wD1Cd", amount: available, subtractFeeFromAmount: true, label: null, message: null }]);
    const est = await cmd.draftEstimate(d);
    expect(est.totalSent + est.fee).toBe(available);
    const others = (await coins()).filter((u) => u.spendable && !u.reserved && !u.userLocked && u.coinjoinRounds === null);
    expect(est.inputCount).toBe(others.length);
  });

  it("coin control refuses it", async () => {
    const d = await cmd.newTxDraft(W);
    await cmd.draftSetRecipients(d, [{ address: "yXdjeVZqmWp4ZcVJ2Hh8hcFhUzFE8wD1Cd", amount: 1_000_000, subtractFeeFromAmount: false, label: null, message: null }]);
    await cmd.draftSetSource(d, { kind: "outpoints", outpoints: [WITHHELD] });
    await expect(cmd.draftEstimate(d)).rejects.toBeTruthy();
  });

  it("the CoinJoin source leaves it out", async () => {
    const status = await cmd.coinjoinStatus(W);
    const b = (await cmd.balance(W))!;
    expect(status.balances.anonymizable + status.balances.fullyMixed).toBe(b.confirmed);
    expect(status.balances.coinjoinAccount).toBe(0);
  });

  it("the Core→Shielded and Platform sources (Core Max) leave it out, even as arriving", () => {
    for (const domain of ["shielded", "credits"] as const) {
      const plan = coreMaxPlan(fixture.world, W, domain, 0);
      const coins = fixture.world.wallets[0].coins;
      const final = coins.filter((u) => u.coinjoinRounds === null && !u.coinjoinDenominated && (u.confirmations > 0 || u.instantLocked));
      expect(plan.totalDuffs).toBe(final.reduce((n, u) => n + u.amount, 0));
      const arriving = coins.filter((u) => u.coinjoinRounds === null && !u.coinjoinDenominated && !final.includes(u) && !isWithheld(u.outpoint));
      expect(plan.max.arrivingDuffs).toBe(arriving.reduce((n, u) => n + u.amount, 0));
    }
  });

  it("abandoning the payment takes the pending coin away, and nothing is given back yet", async () => {
    const before = (await cmd.balance(W))!;
    await cmd.abandonTransaction(W, TXID);
    expect((await coins()).some((u) => isWithheld(u.outpoint))).toBe(false);
    const after = (await cmd.balance(W))!;
    expect(after.withheld).toBe(0);
    expect({ ...after, withheld: CHANGE }).toEqual(before);
  });
});

describe("the coin row and the payment's details (condition (e))", () => {
  const withheldCoin = async () => (await coins()).find((u) => isWithheld(u.outpoint))!;

  it("the coin row says why the coin can't be picked", async () => {
    const u = await withheldCoin();
    expect(notSelectable(u)).toBe(PENDING_TEXT);
    const html = renderToStaticMarkup(
      createElement(CoinTable, {
        coins: [u],
        shown: display({ unit: "dash", decimalDigits: 2 }, { kind: "testnet", dirName: "testnet", unitName: "tDASH" }),
        selected: new Set<string>(),
        onToggle: () => {},
      }),
    );
    expect(html).toContain(PENDING_TEXT.replace(/'/g, "&#x27;"));
    expect(html).toMatch(/<input[^>]*disabled=""/);
  });

  it("another unspendable coin keeps its own reason", async () => {
    const u = await withheldCoin();
    expect(notSelectable({ ...u, excluded: null })).toBe(en.pay.notSelectable.reserved);
  });

  it("the payment's details show the pending line and, beside Abandon, the hint", async () => {
    const x = await cmd.txDetailExtras(W, TXID);
    expect(x.withheldOutputs).toEqual([WITHHELD]);
    expect(x.canAbandon).toBe(true);
    expect(withholds(x)).toBe(true);
    const line = renderToStaticMarkup(createElement(PendingUnprovenLine, { x }));
    expect(line).toContain('data-testid="tx-detail-pending-unproven"');
    expect(line).toContain(PENDING_TEXT.replace(/'/g, "&#x27;"));
    const hint = renderToStaticMarkup(createElement(UnprovenAbandonHint, { x }));
    expect(hint).toContain('data-testid="tx-abandon-unproven-hint"');
    expect(hint).toContain(HINT_TEXT.replace(/'/g, "&#x27;"));
  });

  it("another payment's details show neither", async () => {
    const other = fixture.world.wallets[0].rows.find((r) => r.txid !== TXID && r.category === "sent")!;
    const x: TxDetailExtrasInfo = await cmd.txDetailExtras(W, other.txid);
    expect(x.withheldOutputs).toEqual([]);
    expect(renderToStaticMarkup(createElement(PendingUnprovenLine, { x }))).toBe("");
    expect(renderToStaticMarkup(createElement(UnprovenAbandonHint, { x: { ...x, canAbandon: true } }))).toBe("");
  });

  it("no hint once Abandon isn't offered", async () => {
    const x = await cmd.txDetailExtras(W, TXID);
    expect(renderToStaticMarkup(createElement(UnprovenAbandonHint, { x: { ...x, canAbandon: false } }))).toBe("");
  });
});

