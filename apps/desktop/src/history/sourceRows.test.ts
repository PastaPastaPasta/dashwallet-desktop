import { describe, expect, it } from "vitest";
import type { TxRow } from "../api/generated/TxRow";
import type { PlatformActivity } from "../api/generated/PlatformActivity";
import type { ShieldedActivity } from "../api/generated/ShieldedActivity";
import { filterExtra, mergeActivity, platformRow, rowSource, shieldedRow, sourceRowTitle } from "./sourceRows";

const act = (id: string, over: Partial<PlatformActivity> = {}): PlatformActivity => ({
  id,
  kind: "incoming",
  state: "done",
  credits: "30000000000",
  address: "tdash1abc",
  to: null,
  expectedDuffs: null,
  feeCredits: null,
  at: 1000,
  ...over,
});

const shield = (id: string, over: Partial<ShieldedActivity> = {}): ShieldedActivity => ({
  id,
  kind: "received",
  direction: "in",
  credits: "50000000000",
  feeCredits: null,
  counterparty: null,
  memo: "Lunch",
  blockHeight: 12,
  status: "confirmed",
  createdAt: 1000,
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

const ids = (rows: TxRow[]) => rows.map((r) => r.txid);

describe("platform activity rows", () => {
  it("maps incoming, outgoing and a move to history rows", () => {
    const incoming = platformRow(act("a"));
    expect(incoming).toMatchObject({ txid: "platform:incoming:a", txType: "recvWithAddress", category: "received", amount: 30_000_000, address: "tdash1abc" });
    expect(sourceRowTitle(incoming)).toBe("Received on Platform");
    const outgoing = platformRow(act("b", { kind: "outgoing", credits: "2000000000" }));
    expect(outgoing).toMatchObject({ txType: "sendToAddress", category: "sent", amount: -2_000_000 });
    expect(sourceRowTitle(outgoing)).toBe("Sent from Platform");
    const move = platformRow(act("c", { kind: "moveToCore", address: null, to: "ycore", expectedDuffs: 39_998_000, credits: "40000000000" }));
    expect(move).toMatchObject({ txType: "platformTransfer", category: "platform", amount: 39_998_000, address: "ycore" });
    expect(sourceRowTitle(move)).toBe("Moved to Dash wallet");
    expect([rowSource(move), rowSource(core("abc", 1))]).toEqual(["platform", null]);
  });

  it("shows a pending move as pending and a failed one as a problem", () => {
    expect(platformRow(act("p", { state: "pending" })).status.kind).toBe("unconfirmed");
    expect(platformRow(act("f", { state: "failed" })).status.kind).toBe("notAccepted");
  });
});

describe("shielded activity rows", () => {
  it("titles each kind and signs the direction", () => {
    const received = shieldedRow(shield("a"));
    expect(received).toMatchObject({ txid: "shielded:received:a", txType: "recvWithAddress", amount: 50_000_000, label: "Lunch", blockHeight: 12 });
    expect(sourceRowTitle(received)).toBe("Received in Shielded");
    const sent = shieldedRow(shield("b", { kind: "sent", direction: "out", credits: "10000000000", counterparty: "tdash1zxyz" }));
    expect(sent).toMatchObject({ txType: "sendToAddress", amount: -10_000_000, address: "tdash1zxyz" });
    expect(sourceRowTitle(sent)).toBe("Sent from Shielded");
    const add = shieldedRow(shield("c", { kind: "shieldFromCore", direction: "selfTransfer" }));
    expect(add).toMatchObject({ txType: "platformTransfer", category: "platform" });
    expect(sourceRowTitle(add)).toBe("Added to Shielded");
    // E1's contract: `unshield` pays a Platform address, `withdrawal` a Core address.
    const unshield = shieldedRow(shield("d", { kind: "unshield", direction: "out", credits: "10000000000", counterparty: "tdash1platformaddress" }));
    expect(unshield).toMatchObject({ txType: "sendToAddress", category: "sent", amount: -10_000_000, address: "tdash1platformaddress" });
    expect(sourceRowTitle(unshield)).toBe("Sent to Platform");
    const unshieldSelf = shieldedRow(shield("d2", { kind: "unshield", direction: "selfTransfer" }));
    expect([unshieldSelf.category, sourceRowTitle(unshieldSelf)]).toEqual(["platform", "Moved to Platform"]);
    const withdrawal = shieldedRow(shield("w", { kind: "withdrawal", direction: "out", counterparty: "yCoreAddress" }));
    expect([withdrawal.category, sourceRowTitle(withdrawal)]).toEqual(["platform", "Moved to Dash wallet"]);
    // Whatever its kind, a move between the account's own notes is internal: no sign.
    expect(shieldedRow(shield("s", { kind: "spend", direction: "selfTransfer" })).txType).toBe("platformTransfer");
    expect(sourceRowTitle(shieldedRow(shield("e", { kind: "identityTopUp", direction: "out" })))).toBe("Paid from Shielded");
    expect(rowSource(received)).toBe("shielded");
  });

  it("maps the status", () => {
    expect(shieldedRow(shield("p", { status: "pending" })).status.kind).toBe("unconfirmed");
    expect(shieldedRow(shield("u", { status: "unknown" })).status.kind).toBe("unconfirmed");
    expect(shieldedRow(shield("f", { status: "failed" })).status.kind).toBe("notAccepted");
    expect(shieldedRow(shield("n", { createdAt: null })).timestamp).toBeNull();
  });
});

describe("merging into Activity", () => {
  it("merges by date, newest first, the Core row first on a tie", () => {
    const extra = [platformRow(act("p2", { at: 2000 })), platformRow(act("p1", { at: 1000 })), shieldedRow(shield("s", { createdAt: 3500 }))];
    expect(ids(mergeActivity([core("c3", 3000), core("c1", 1000)], false, extra))).toEqual(["shielded:received:s", "c3", "platform:incoming:p2", "c1", "platform:incoming:p1"]);
  });

  it("holds back rows older than the loaded Core rows while more pages exist", () => {
    const extra = [platformRow(act("new", { at: 2500 })), platformRow(act("old", { at: 500 }))];
    expect(ids(mergeActivity([core("c3", 3000), core("c2", 2000)], true, extra))).toEqual(["c3", "platform:incoming:new", "c2"]);
    expect(ids(mergeActivity([core("c3", 3000), core("c2", 2000)], false, extra))).toContain("platform:incoming:old");
  });

  it("applies the date range (end exclusive) and the minimum amount", () => {
    const extra = [platformRow(act("a", { at: 100 })), platformRow(act("b", { at: 200 })), platformRow(act("c", { at: 300, credits: "1000000" }))];
    expect(ids(filterExtra(extra, { dateFrom: 150, dateTo: 300 }))).toEqual(["platform:incoming:b"]);
    expect(ids(filterExtra(extra, { minAmount: 1_000_000 }))).toEqual(["platform:incoming:a", "platform:incoming:b"]);
    expect(filterExtra(extra, {})).toHaveLength(3);
  });

  it("treats a row without a time as the newest, as the engine does", () => {
    const undated = { ...core("mempool", 0), timestamp: null };
    const sh = shieldedRow(shield("n", { createdAt: null }));
    // Stays on top among the Core rows, and a Shielded row without a time passes the page cut.
    expect(ids(mergeActivity([undated, core("c3", 3000), core("c2", 2000)], true, [platformRow(act("p", { at: 2500 })), sh]))).toEqual(["mempool", "shielded:received:n", "c3", "platform:incoming:p", "c2"]);
    // A date range drops what has no time, as it drops Core's.
    expect(filterExtra([sh], { dateFrom: 1 })).toEqual([]);
    expect(filterExtra([sh], {})).toEqual([sh]);
  });

  it("returns the Core rows as they are without extra rows", () => {
    const rows = [core("c1", 1)];
    expect(mergeActivity(rows, false, [])).toBe(rows);
  });
});
