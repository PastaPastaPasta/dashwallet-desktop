import { afterEach, describe, expect, it, vi } from "vitest";
import type { TxNoticeRow } from "../api/generated/TxNoticeRow";
import { announceReceived, isReceivedPayment, markReceivedRead, onReceived, recordReceived, resetReceived, unreadReceived } from "./received";

const row = (over: Partial<TxNoticeRow> = {}): TxNoticeRow => ({
  txid: "aa".repeat(32),
  recordIndex: 0,
  amount: 999_819,
  timestamp: 1,
  txType: "recvWithAddress",
  address: "yAddr",
  label: null,
  coinjoinInternal: false,
  ...over,
});

afterEach(resetReceived);

describe("received payments (DEC-1058 #6)", () => {
  it("DEC1058_onlyMoneyPaidToTheWalletIsAnnounced", () => {
    expect(isReceivedPayment(row())).toBe(true);
    expect(isReceivedPayment(row({ txType: "recvFromOther" }))).toBe(true);
    expect(isReceivedPayment(row({ amount: -5 }))).toBe(false);
    expect(isReceivedPayment(row({ amount: 0 }))).toBe(false);
    expect(isReceivedPayment(row({ txType: "recvWithCoinJoin" }))).toBe(false);
    expect(isReceivedPayment(row({ txType: "dustReceive" }))).toBe(false);
    expect(isReceivedPayment(row({ txType: "generated" }))).toBe(false);
    expect(isReceivedPayment(row({ coinjoinInternal: true }))).toBe(false);
  });

  it("records each payment once, tells the listeners and counts it unread until the bell was opened", () => {
    const seen = vi.fn();
    const stop = onReceived(seen);
    const batch = recordReceived("w1", [row(), row({ txid: "bb".repeat(32), amount: -1 })], 100);
    expect(batch).toEqual([{ id: `${"aa".repeat(32)}:0`, walletId: "w1", txid: "aa".repeat(32), duffs: 999_819, address: "yAddr", at: 100, read: false }]);
    expect(seen).toHaveBeenCalledTimes(1);
    // The same payment again (a repeated event) is not news.
    expect(recordReceived("w1", [row()])).toEqual([]);
    expect(seen).toHaveBeenCalledTimes(1);
    // Another wallet's is its own.
    expect(recordReceived("w2", [row()])).toHaveLength(1);
    stop();
    expect(recordReceived("w1", [row({ txid: "cc".repeat(32) })])).toHaveLength(1);
    expect(seen).toHaveBeenCalledTimes(2);
    markReceivedRead("w1");
    expect(unreadReceived([{ ...batch[0], read: true }])).toBe(0);
  });

  it("asks the engine for the rows, and a failed read announces nothing", async () => {
    const txNotices = vi.fn().mockResolvedValue([row()]);
    const seen = vi.fn();
    onReceived(seen);
    await announceReceived({ txNotices }, "w1", ["t"]);
    expect(txNotices).toHaveBeenCalledWith("w1", ["t"]);
    expect(seen).toHaveBeenCalledTimes(1);
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    await announceReceived({ txNotices: vi.fn().mockRejectedValue(new Error("x")) }, "w1", ["t"]);
    expect(seen).toHaveBeenCalledTimes(1);
    warn.mockRestore();
  });
});
