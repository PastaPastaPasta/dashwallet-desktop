// Coin Selection, by parity ID (the Swift `CoinControlViewModelTests`, ported). Sorting is per
// session here, not persisted (U-05 D8).

import { describe, expect, it, vi } from "vitest";
import type { CoinSelectionSummary } from "../api/generated/CoinSelectionSummary";
import type { OutPoint } from "../api/generated/OutPoint";
import type { Utxo } from "../api/generated/Utxo";
import { appError, settle, WALLET_A } from "../onboarding/testWorld";
import { notSelectable, outpointKey } from "../screens/pay/model";
import { COLUMN_TITLE, CoinSelectionModel, CoinText, coinLabel, copyText } from "./coins";

const txid = (n: number) => n.toString(16).padStart(64, "0");
const key = (n: number) => `${txid(n)}:0`;
const A1 = "yAddressOne0000000000000000000001";
const A2 = "yAddressTwo0000000000000000000002";

function utxo(n: number, amount: number, patch: Partial<Utxo> = {}): Utxo {
  return {
    outpoint: { txid: txid(n), vout: 0 },
    address: A1,
    amount,
    confirmations: 10,
    blockHeight: 100,
    timestamp: 1_700_000_000 + n,
    instantLocked: false,
    chainLocked: true,
    userLocked: false,
    reserved: false,
    label: null,
    isChange: false,
    isCoinbase: false,
    coinjoinDenominated: false,
    coinjoinRounds: null,
    spendable: true,
    ...patch,
  };
}

const summaryOf = (coins: Utxo[], spent: Set<string>, outpoints: OutPoint[]): CoinSelectionSummary => {
  const live = outpoints.filter((o) => !spent.has(outpointKey(o)));
  const amount = live.reduce((t, o) => t + (coins.find((u) => outpointKey(u.outpoint) === outpointKey(o))?.amount ?? 0), 0);
  // 148·inputs + 34·(outputs + change) + 10 bytes at 1000 duff/kB, as the engine computes it.
  const bytes = 148 * live.length + 34 + 10;
  return {
    quantity: live.length,
    amount,
    bytes,
    fee: bytes,
    afterFee: amount - bytes,
    change: 0,
    changeToFee: false,
    insufficientFunds: false,
    feeTolerancePerInput: 2,
    unavailable: outpoints.filter((o) => spent.has(outpointKey(o))),
  };
};

/** The engine's coins and locks; `reload` reads them as `utxos` would. */
function selection(initial: Utxo[], locked: string[] = []) {
  const locks = new Set(locked);
  const spent = new Set<string>();
  const coins = () =>
    initial.map((u) => {
      const userLocked = locks.has(outpointKey(u.outpoint));
      return { ...u, userLocked, spendable: u.spendable && !userLocked };
    });
  const api = {
    lockOutpoints: vi.fn(async (_: string, os: OutPoint[]) => {
      os.forEach((o) => locks.add(outpointKey(o)));
      return null;
    }),
    unlockOutpoints: vi.fn(async (_: string, os: OutPoint[]) => {
      os.forEach((o) => locks.delete(outpointKey(o)));
      return null;
    }),
    coinSelectionSummary: vi.fn(async (_: string, os: OutPoint[]) => summaryOf(initial, spent, os)),
  };
  const m = new CoinSelectionModel(api, WALLET_A, async () => coins());
  m.setCoins(coins());
  return { m, api, locks, spent, coins };
}

describe("Coin Selection (QT-069…074)", () => {
  it("QT069_listModeDefaultSortedByAmountDescending", () => {
    const { m } = selection([utxo(1, 100_000), utxo(2, 900_000), utxo(3, 500_000)]);
    expect(m.mode).toBe("list");
    expect(m.visible.map((u) => u.amount)).toEqual([900_000, 500_000, 100_000]);
    m.sortBy("amount");
    expect(m.visible.map((u) => u.amount)).toEqual([100_000, 500_000, 900_000]);
    m.sortBy("confirmations");
    expect(m.sort).toEqual({ column: "confirmations", ascending: true });
    m.setMode("tree");
    expect(m.mode).toBe("tree");
    expect(COLUMN_TITLE.label).toBe("Label");
  });

  it("QT069_treeModeGroupsByAddressAndLabelsChange", () => {
    const { m } = selection([
      utxo(1, 100, { label: "Salary" }),
      utxo(2, 200, { label: "Salary" }),
      utxo(3, 300, { address: A2, isChange: true }),
    ]);
    m.setMode("tree");
    expect(m.groups).toHaveLength(2);
    expect(m.groups.find((g) => g.address === A1)).toMatchObject({ label: "Salary", total: 300 });
    expect(coinLabel(m.visible.find((u) => u.isChange)!)).toBe(CoinText.change);
    expect(coinLabel(utxo(9, 1))).toBe(CoinText.noLabel);
  });

  it("QT070_copyMenuAndLocks", async () => {
    const { m } = selection([utxo(1, 123_456_789, { label: "Rent" })]);
    const u = m.visible[0];
    expect(copyText(u, "outpoint")).toBe(key(1));
    expect(copyText(u, "amount")).toBe("1.23456789");
    expect(copyText(u, "label")).toBe("Rent");
    expect(copyText(u, "address")).toBe(A1);
    m.toggle(key(1));
    expect(m.selected).toEqual([key(1)]);
    await m.lock([key(1)]);
    expect([m.lockedCount, m.lockedText]).toEqual([1, "(1 locked)"]);
    expect(m.selected).toEqual([]);
    expect(m.unselectedNotice).toBe(false);
    m.toggle(key(1));
    expect(m.selected).toEqual([]); // locked coins cannot be picked
    expect(notSelectable(m.visible[0])).toBe("Locked");
    await m.unlock([key(1)]);
    expect([m.lockedCount, m.lockedText]).toEqual([0, null]);
  });

  it("QT070_toggleLockStateFlipsEveryLock (the Swift QT070_unlockAllFlipsEveryLock)", async () => {
    const { m, locks } = selection([utxo(1, 1000), utxo(2, 2000)], [key(1)]);
    await m.toggleLocks();
    expect([...locks]).toEqual([key(2)]);
  });

  it("QT070_aReservedCoinIsNeverLockedByTheToggle", async () => {
    const { m, locks } = selection([utxo(1, 1000, { reserved: true, spendable: false }), utxo(2, 2000)]);
    await m.toggleLocks();
    expect([...locks]).toEqual([key(2)]);
  });

  it("QT070_aFailedLockShowsTheErrorAndKeepsTheSelection", async () => {
    const { m, api } = selection([utxo(1, 1000)]);
    m.toggle(key(1));
    api.lockOutpoints.mockRejectedValueOnce(appError("wallet_not_found"));
    await m.lock([key(1)]);
    expect(m.error).toBe("wallet_not_found");
    expect(m.selected).toEqual([key(1)]);
  });

  it("QT071_coinJoinCoinsAreHiddenByDefault", () => {
    const { m } = selection([utxo(1, 1000), utxo(2, 100_001, { coinjoinDenominated: true, coinjoinRounds: 4 })]);
    expect(m.visible).toHaveLength(1);
    expect(m.coinJoinToggleTitle).toBe(CoinText.showAll);
    m.setShowCoinJoin(true);
    expect(m.visible).toHaveLength(2);
    expect(m.coinJoinToggleTitle).toBe(CoinText.hideCoinJoin);
  });

  it("QT072_summaryUsesTheEngineFormula", async () => {
    const { m, api } = selection([utxo(1, 10_000_000), utxo(2, 5_000_000)]);
    m.toggleAll();
    await vi.waitFor(() => expect(m.summary).not.toBeNull());
    expect(api.coinSelectionSummary).toHaveBeenLastCalledWith(WALLET_A, expect.any(Array), [], { kind: "recommended", targetBlocks: 6 });
    expect(api.coinSelectionSummary.mock.lastCall?.[1]).toHaveLength(2);
    expect(m.summary).toMatchObject({ quantity: 2, amount: 15_000_000, bytes: 340, fee: 340, afterFee: 14_999_660 });
    expect(CoinText.tolerance(m.summary!.feeTolerancePerInput)).toBe("Can vary +/- 2 duffs per input.");
    m.toggleAll();
    await settle();
    expect(m.summary).toBeNull();
  });

  it("QT072_summaryNotImplementedIsUnavailableNotZero", async () => {
    const { m, api } = selection([utxo(1, 10_000_000)]);
    api.coinSelectionSummary.mockRejectedValueOnce(appError("not_implemented"));
    m.toggle(key(1));
    await vi.waitFor(() => expect(m.summaryUnavailable).toBe(true));
    expect(m.summary).toBeNull();
    expect(m.error).toBeNull();
  });

  it("QT074_spentSelectionsAreUnselectedWithTheNotice", async () => {
    const { m, spent } = selection([utxo(1, 10_000_000), utxo(2, 5_000_000)]);
    m.toggleAll();
    await vi.waitFor(() => expect(m.summary?.quantity).toBe(2));
    expect(m.unselectedNotice).toBe(false);
    // Coin 2 is spent elsewhere: the engine reports it unavailable.
    spent.add(key(2));
    m.toggle(key(1));
    m.toggle(key(1));
    await vi.waitFor(() => expect(m.unselectedNotice).toBe(true));
    expect(m.selected).toEqual([key(1)]);
    expect(CoinText.unselected).toBe("Some coins were unselected because they were spent.");
  });

  it("QT074_aCoinGoneFromTheListIsUnselectedWithTheNotice", () => {
    const { m, coins } = selection([utxo(1, 1000), utxo(2, 2000)]);
    m.toggleAll();
    m.setCoins(coins().slice(0, 1));
    expect(m.selected).toEqual([key(1)]);
    expect(m.unselectedNotice).toBe(true);
    m.toggle(key(1));
    expect(m.unselectedNotice).toBe(false);
  });

  it("toggleAll picks only the selectable coins shown", () => {
    const { m } = selection([utxo(1, 1000), utxo(2, 2000, { isCoinbase: true, spendable: false }), utxo(3, 3000, { coinjoinDenominated: true })]);
    m.toggleAll();
    expect(m.selected).toEqual([key(1)]);
    m.toggleAll();
    expect(m.selected).toEqual([]);
  });

  it("QT069_aTreeAddressRowTogglesOnlyItsCoins", () => {
    const { m } = selection([utxo(1, 1000), utxo(2, 2000, { address: A2 }), utxo(3, 3000)]);
    m.toggle(key(2));
    const one = m.groups.find((g) => g.address === A1)!;
    m.toggleCoins(one.coins);
    expect(m.selected.toSorted()).toEqual([key(1), key(2), key(3)].toSorted());
    m.toggleCoins(one.coins);
    expect(m.selected).toEqual([key(2)]);
  });
});
