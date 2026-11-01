import { describe, expect, it } from "vitest";
import type { TxRow } from "../api/generated/TxRow";
import type { PlatformActivity } from "../api/pending";
import { isPlatformRow, mergeActivity, platformRow, platformRowTitle } from "./platformRows";

const act = (id: string, over: Partial<PlatformActivity> = {}): PlatformActivity => ({
  id,
  kind: "incoming",
  state: "done",
  credits: "30000000000",
  address: "tdash1abc",
  to: null,
  expectedDuffs: null,
  at: 1000,
  ...over,
});

const core = (txid: string, timestamp: number, amount = 5): TxRow => ({
  txid,
  recordIndex: 0,
  txType: "recvWithAddress",
  category: "received",
  status: { kind: "confirmed", confirmations: 6, instantLocked: false, chainLocked: true, maturesIn: null },
  timestamp,
  blockHeight: 1,
  amount,
  fee: null,
  address: null,
  label: null,
  countsTowardBalance: true,
  involvesWatchOnly: false,
});

describe("platform activity rows", () => {
  it("maps incoming, outgoing and a move to history rows", () => {
    const incoming = platformRow(act("a"));
    expect(incoming).toMatchObject({ txid: "platform:a", txType: "recvWithAddress", category: "received", amount: 30_000_000, address: "tdash1abc" });
    expect(platformRowTitle(incoming)).toBe("Received on Platform");
    const outgoing = platformRow(act("b", { kind: "outgoing", credits: "2000000000" }));
    expect(outgoing).toMatchObject({ txType: "sendToAddress", category: "sent", amount: -2_000_000 });
    expect(platformRowTitle(outgoing)).toBe("Sent from Platform");
    const move = platformRow(act("c", { kind: "moveToCore", address: null, to: "ycore", expectedDuffs: 39_998_000, credits: "40000000000" }));
    expect(move).toMatchObject({ txType: "platformTransfer", category: "platform", amount: 39_998_000, address: "ycore" });
    expect(platformRowTitle(move)).toBe("Moved to Dash wallet");
    expect(isPlatformRow(move)).toBe(true);
    expect(isPlatformRow(core("abc", 1))).toBe(false);
  });

  it("shows a pending move as pending and a failed one as a problem", () => {
    expect(platformRow(act("p", { state: "pending" })).status.kind).toBe("unconfirmed");
    expect(platformRow(act("f", { state: "failed" })).status.kind).toBe("notAccepted");
  });

  it("merges by date, newest first, the Core row first on a tie", () => {
    const rows = mergeActivity([core("c3", 3000), core("c1", 1000)], false, [act("p2", { at: 2000 }), act("p1", { at: 1000 })]);
    expect(rows.map((r) => r.txid)).toEqual(["c3", "platform:p2", "c1", "platform:p1"]);
  });

  it("holds back Platform rows older than the loaded Core rows while more pages exist", () => {
    const rows = mergeActivity([core("c3", 3000), core("c2", 2000)], true, [act("new", { at: 2500 }), act("old", { at: 500 })]);
    expect(rows.map((r) => r.txid)).toEqual(["c3", "platform:new", "c2"]);
    expect(mergeActivity([core("c3", 3000), core("c2", 2000)], false, [act("old", { at: 500 })]).map((r) => r.txid)).toContain("platform:old");
  });

  it("applies the date range (end exclusive) and the minimum amount", () => {
    const list = [act("a", { at: 100 }), act("b", { at: 200 }), act("c", { at: 300, credits: "1000000" })];
    expect(mergeActivity([], false, list, { dateFrom: 150, dateTo: 300 }).map((r) => r.txid)).toEqual(["platform:b"]);
    expect(mergeActivity([], false, list, { minAmount: 1_000_000 }).map((r) => r.txid)).toEqual(["platform:b", "platform:a"]);
  });

  it("returns the Core rows as they are without activity", () => {
    const rows = [core("c1", 1)];
    expect(mergeActivity(rows, false, null)).toBe(rows);
    expect(mergeActivity(rows, false, [])).toBe(rows);
  });
});
