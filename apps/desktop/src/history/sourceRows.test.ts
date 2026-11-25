import { describe, expect, it } from "vitest";
import type { TxRow } from "../api/generated/TxRow";
import type { PlatformActivity } from "../api/generated/PlatformActivity";
import type { ShieldedActivity } from "../api/generated/ShieldedActivity";
import { filterExtra, findSourceRecord, isMayLandRow, isReplacedRow, isSettledUnknownRow, mergeActivity, platformRow, rowSource, settledUnknownText, shieldedRow, sourceRowTitle } from "./sourceRows";

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
  mayLand: false,
  estimated: false,
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
  it("shows an add from the Dash wallet as a receipt on Platform", () => {
    const done = platformRow(act("f", { kind: "fromCore", credits: "49944000000" }));
    expect([rowSource(done), sourceRowTitle(done), done.category, done.txType, done.amount, done.address, done.status]).toEqual(["platform", "Moved from Dash wallet", "received", "recvWithAddress", 49_944_000, "tdash1abc", done.status]);
    const pending = platformRow(act("g", { kind: "fromCore", state: "pending", estimated: true }));
    expect(pending.status).not.toBe(done.status);
    expect(isMayLandRow(pending)).toBe(false);
  });

  it("shows an unshield to an own Platform address as From Shielded, a receipt there", () => {
    const done = platformRow(act("s", { kind: "fromShielded", credits: "25000000000" }));
    expect([sourceRowTitle(done), done.category, done.txType, done.amount, done.address]).toEqual(["From Shielded", "received", "recvWithAddress", 25_000_000, "tdash1abc"]);
    const pending = platformRow(act("t", { kind: "fromShielded", state: "pending", mayLand: true }));
    expect(pending.status).not.toBe(done.status);
    expect(isMayLandRow(pending)).toBe(false);
    const failed = platformRow(act("u", { kind: "fromShielded", state: "failed" }));
    expect([sourceRowTitle(failed), failed.status.kind]).toEqual(["From Shielded", "notAccepted"]);
  });

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

  it("shows a send to a Platform address as a payment to its recipient", () => {
    const send = platformRow(act("s", { kind: "send", address: null, to: "tdash1kpayee", credits: "1006500000" }));
    expect(send).toMatchObject({ txid: "platform:send:s", txType: "sendToAddress", category: "sent", amount: -1_006_500, address: "tdash1kpayee" });
    expect(sourceRowTitle(send)).toBe("Sent to a Platform address");
    expect(platformRow(act("sp", { kind: "send", state: "pending", to: "tdash1kpayee" })).status.kind).toBe("unconfirmed");
  });

  it("shows a pending move as pending and a failed one as a problem", () => {
    expect(platformRow(act("p", { state: "pending" })).status.kind).toBe("unconfirmed");
    expect(platformRow(act("f", { state: "failed" })).status.kind).toBe("notAccepted");
  });

  it("shows a failed send or move that may still land as unconfirmed with its amount, never as not accepted", () => {
    const send = platformRow(act("s", { kind: "send", state: "failed", mayLand: true, address: null, to: "tdash1kpayee", credits: "1019500000" }));
    expect(send).toMatchObject({ txid: "platform:send:s:mayland", amount: -1_019_500, status: { kind: "unconfirmed" }, address: "tdash1kpayee" });
    expect(isMayLandRow(send)).toBe(true);
    expect(sourceRowTitle(send)).toBe("Sent to a Platform address");
    const move = platformRow(act("m", { kind: "moveToCore", state: "failed", mayLand: true, to: "ycore", expectedDuffs: null, credits: "40000000000" }));
    expect(move).toMatchObject({ status: { kind: "unconfirmed" }, amount: 40_000_000 });
    expect(isMayLandRow(move)).toBe(true);
    // One that never will stays a problem; a pending one is a plain pending row.
    const gone = platformRow(act("g", { kind: "send", state: "failed", mayLand: false, to: "tdash1kpayee" }));
    expect([gone.status.kind, isMayLandRow(gone)]).toEqual(["notAccepted", false]);
    expect(isMayLandRow(platformRow(act("p", { kind: "send", state: "pending", mayLand: true, to: "tdash1kpayee" })))).toBe(false);
  });

  it("shows a replaced send like a replaced move: no amount, no problem", () => {
    const row = platformRow(act("r", { kind: "send", state: "replaced", to: "tdash1kpayee" }));
    expect(row).toMatchObject({ amount: 0, status: { kind: "confirmed" } });
    expect(isReplacedRow(row)).toBe(true);
  });

  it("shows a replaced move as neither a problem nor an amount", () => {
    const row = platformRow(act("r", { kind: "moveToCore", state: "replaced", expectedDuffs: null, to: null, address: null }));
    expect(row).toMatchObject({ amount: 0, status: { kind: "confirmed" } });
    expect([isReplacedRow(row), isReplacedRow(platformRow(act("d", { kind: "moveToCore" })))]).toEqual([true, false]);
    expect(rowSource(row)).toBe("platform");
  });
});

describe("a group that settled with no named winner", () => {
  const unknown = (id: string, over: Partial<PlatformActivity>) => platformRow(act(id, { address: null, winnerKnown: false, ...over }));
  const MOVE = "This move and another payment or move from the same Platform addresses couldn't both go through, and only one did. Dash Wallet can't tell which. Check your Platform balance and Dash wallet before moving again.";
  const PAY = "This payment and another payment or move from the same Platform addresses couldn't both go through, and only one did. Dash Wallet can't tell which. Before paying again, check whether the recipient was paid.";
  const TOP_UP = "This top-up and another payment or move from the same Platform addresses couldn't both go through, and only one did. Dash Wallet can't tell which. Check your identity's balance before topping up again.";

  it("titles every row of each kind as settled with another, with the kind's text", () => {
    const rows = [
      unknown("m", { kind: "moveToCore", state: "done" }),
      unknown("s", { kind: "send", state: "replaced", to: "tdash1kpayee" }),
      unknown("c", { kind: "contactPayment", state: "replaced", to: "tdash1kpayee" }),
      unknown("t", { kind: "toIdentity", state: "done" }),
      unknown("x", { kind: "moveToCore", state: "failed", mayLand: true }),
    ];
    expect(rows.map(sourceRowTitle)).toEqual(Array(5).fill("Settled with another payment or move"));
    expect(rows.map((r) => settledUnknownText(r).text)).toEqual([MOVE, PAY, PAY, TOP_UP, MOVE]);
    expect(rows.map((r) => [isSettledUnknownRow(r), isReplacedRow(r), isMayLandRow(r), r.status.kind])).toEqual(Array(5).fill([true, false, false, "confirmed"]));
  });

  it("puts the group's debit on its newest (done) row only", () => {
    const newest = unknown("m", { kind: "moveToCore", state: "done", credits: "30000000000" });
    const older = unknown("s", { kind: "send", state: "replaced", credits: "25000000000", to: "tdash1kpayee" });
    const exit = unknown("x", { kind: "moveToCore", state: "failed", credits: "20000000000" });
    expect([newest.amount, older.amount, exit.amount]).toEqual([30_000_000, 0, 0]);
    const topUp = unknown("t", { kind: "toIdentity", state: "done", credits: "20000000000" });
    expect(topUp.amount).toBe(-20_000_000);
  });

  it("keeps a group with a named winner as it was", () => {
    const replaced = platformRow(act("r", { kind: "moveToCore", state: "replaced", address: null }));
    expect([isSettledUnknownRow(replaced), isReplacedRow(replaced), sourceRowTitle(replaced)]).toEqual([false, true, "Moved to Dash wallet"]);
  });

  it("finds a settled row's record", () => {
    const record = act("m", { kind: "moveToCore", winnerKnown: false });
    expect(findSourceRecord(platformRow(record).txid, [record], [])?.record).toBe(record);
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

  it("titles a contact payment by its source: Paid from Platform", () => {
    const row = platformRow(act("cp", { kind: "contactPayment", state: "done", to: "yXcontact", contact: { identity: "bob", contact: "me" } as never }));
    expect([sourceRowTitle(row), row.amount < 0]).toEqual(["Paid from Platform", true]);
    expect(sourceRowTitle(shieldedRow(shield("sp", { kind: "spend", direction: "out" })))).toBe("Paid from Shielded");
  });

  it("keeps a contact payment that failed by height but may still land unconfirmed and outgoing, at its recipient", () => {
    const contact = { identity: "bob", contact: "me" } as never;
    const held = platformRow(act("cq", { kind: "contactPayment", state: "failed", mayLand: true, address: "tdash1kfrom", to: "yXcontact", credits: "1019500000", contact }));
    expect(held).toMatchObject({ txid: "platform:contactPayment:cq:mayland", amount: -1_019_500, status: { kind: "unconfirmed" }, address: "yXcontact" });
    expect(isMayLandRow(held)).toBe(true);
    const gone = platformRow(act("cr", { kind: "contactPayment", state: "failed", mayLand: false, to: "yXcontact", contact }));
    expect([gone.status.kind, isMayLandRow(gone)]).toEqual(["notAccepted", false]);
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

describe("findSourceRecord", () => {
  const platform = [act("pa-1"), act("pm-1", { kind: "moveToCore", state: "failed", mayLand: true, address: null, to: "yCore" })];
  const shielded = [shield("sh-1"), shield("sh-2", { kind: "sent", direction: "out", memo: null })];

  it("finds the record a row was made from, with that row", () => {
    const found = findSourceRecord(shieldedRow(shielded[0]).txid, platform, shielded);
    expect(found?.source).toBe("shielded");
    expect(found?.record).toBe(shielded[0]);
    expect(found?.row).toEqual(shieldedRow(shielded[0]));
    expect(findSourceRecord("platform:incoming:pa-1", platform, shielded)?.record).toBe(platform[0]);
  });

  it("keeps a move whose state changed while its details are open", () => {
    expect(findSourceRecord("platform:moveToCore:pm-1:mayland", platform, shielded)?.record).toBe(platform[1]);
    const replaced = [act("pm-1", { kind: "moveToCore", state: "replaced", address: null })];
    expect(findSourceRecord("platform:moveToCore:pm-1:mayland", replaced, shielded)?.record).toBe(replaced[0]);
  });

  it("finds nothing for a Core txid, another source, or a record the read no longer has", () => {
    expect(findSourceRecord("ab".repeat(32), platform, shielded)).toBeNull();
    expect(findSourceRecord("shielded:received:pa-1", platform, shielded)).toBeNull();
    expect(findSourceRecord("shielded:received:sh-9", platform, shielded)).toBeNull();
    expect(findSourceRecord("platform:incoming:pa-1", undefined, shielded)).toBeNull();
  });
});
