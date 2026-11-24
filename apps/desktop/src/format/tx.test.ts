import { describe, expect, it } from "vitest";
import type { TxStatusInfo } from "../api/generated/TxStatusInfo";
import { contactAside, contactLetters, contactName, dateRange, dayTitle, detailWhat, detailWho, groupAmount, groupByDay, groupChip, groupConsecutiveMixing, rowParty, shortAddress, shownAmount, tableTypeTitle, txChip, txIcon, txTitle, typeTitle } from "./tx";

const status = (over: Partial<TxStatusInfo> = {}): TxStatusInfo => ({
  kind: "confirmed",
  confirmations: 10,
  instantLocked: false,
  chainLocked: true,
  maturesIn: null,
  ...over,
});

describe("titles", () => {
  it("prefers the label and never shows an address", () => {
    expect(txTitle({ txType: "recvWithAddress", category: "received", amount: 1, label: "Coffee refund" })).toBe("Coffee refund");
    expect(txTitle({ txType: "recvWithAddress", category: "received", amount: 1, label: null })).toBe("Received");
  });

  it("names a DashPay counterparty after the label, never by its identity id (DP3-02)", () => {
    const bob = { identity: "8GopL", username: "bob", displayName: "Bob Builder", avatarUrl: null };
    const recv = { txType: "recvWithAddress", category: "received", amount: 1, label: null } as const;
    expect(contactName(bob)).toBe("Bob Builder");
    expect(contactName({ ...bob, displayName: null })).toBe("@bob");
    expect(contactName({ ...bob, displayName: "", username: null })).toBeNull();
    expect(contactName({ ...bob, displayName: "  Bob  " })).toBe("Bob");
    expect(contactName({ ...bob, displayName: " \t " })).toBe("@bob");
    expect(contactName(undefined)).toBeNull();
    expect(txTitle({ ...recv, counterparty: bob })).toBe("Bob Builder");
    expect(txTitle({ ...recv, counterparty: { ...bob, displayName: null, username: null } })).toBe("Received");
    expect(txTitle({ ...recv, label: "Rent", counterparty: bob })).toBe("Rent");
    expect(rowParty({ label: null, address: "yAddr", counterparty: { ...bob, displayName: null } })).toBe("@bob");
    expect(rowParty({ label: "Rent", address: "yAddr", counterparty: bob })).toBe("Rent");
    expect(rowParty({ label: null, address: "yAddr", counterparty: { ...bob, displayName: null, username: null } })).toBe("yAddr");
  });

  it("IOS-029 a named counterparty's avatar letters; none without a name, so the type icon stays (DP3-07 D1)", () => {
    const bob = { identity: "8GopL", username: "bob", displayName: "Bob Builder", avatarUrl: null };
    expect(contactLetters(bob)).toBe("BB");
    expect(contactLetters({ ...bob, displayName: null })).toBe("B");
    expect(contactLetters({ ...bob, displayName: " \t " })).toBe("B");
    expect(contactLetters({ ...bob, displayName: null, username: null })).toBeNull();
    expect(contactLetters({ ...bob, displayName: "", username: null })).toBeNull();
    expect(contactLetters(undefined)).toBeNull();
    // A name without a letter says less than the type icon.
    expect(contactLetters({ ...bob, displayName: "🚀 123" })).toBeNull();
  });

  it("names the contact aloud when a label takes the title (DP3-07 D13)", () => {
    const bob = { identity: "8GopL", username: "bob", displayName: "Bob Builder", avatarUrl: null };
    expect(contactAside({ amount: 5, counterparty: bob }, "Coffee refund")).toBe("from Bob Builder");
    expect(contactAside({ amount: -5, counterparty: bob }, "Lunch")).toBe("to Bob Builder");
    expect(contactAside({ amount: -5, counterparty: { ...bob, displayName: null } }, "Lunch")).toBe("to @bob");
    // The title already names the contact, or there is no name to say.
    expect(contactAside({ amount: 5, counterparty: bob }, "Bob Builder")).toBeNull();
    expect(contactAside({ amount: 5, counterparty: { ...bob, displayName: null, username: null } }, "Received")).toBeNull();
    expect(contactAside({ amount: 5, counterparty: undefined }, "Coffee refund")).toBeNull();
  });

  it("names who and what happened with the activity row's words", () => {
    const bob = { identity: "8GopL", username: "bob", displayName: "Bob Builder", avatarUrl: null };
    const recv = { txType: "recvWithAddress", category: "received", amount: 1, label: null, address: "y" + "a".repeat(33) } as const;
    expect(detailWhat(recv)).toBe("Received");
    expect(detailWho(recv)).toBe(shortAddress(recv.address));
    expect(detailWho({ ...recv, counterparty: bob })).toBe("Bob Builder");
    expect(detailWhat(recv)).toBe("Received");
    expect(detailWho({ ...recv, label: "Alice" })).toBe("Alice");
    expect(detailWhat({ txType: "sendToAddress", category: "sent", amount: -1 })).toBe("Sent");
    expect(detailWhat({ txType: "coinJoinMixing", category: "coinJoin", amount: 0 })).toBe("Mixing");
    expect(detailWhat({ txType: "sendToSelf", category: "internalTransfer", amount: -226 })).toBe("Sent to yourself");
    const lock = { txType: "assetLock", category: "platform", amount: -1, purpose: { kind: "usernameRegistration", username: "dwdemo2611" } } as const;
    expect(detailWhat(lock)).toBe("Username @dwdemo2611");
    expect(detailWho({ label: "  ", address: null, counterparty: undefined })).toBeNull();
  });

  it("maps dash-qt types to iOS titles", () => {
    expect(typeTitle("coinJoinMixing", "coinJoin", 0)).toBe("Mixing");
    expect(typeTitle("recvWithAddress", "reward", 5)).toBe("Masternode reward");
    expect(typeTitle("generated", "reward", 5)).toBe("Mined");
    expect(typeTitle("other", "other", -1)).toBe("Sent");
    expect(typeTitle("assetLock", "platform", -1)).toBe("Dash Platform funding");
    const lock = { txType: "assetLock", category: "platform", amount: -1, label: null } as const;
    expect(txTitle({ ...lock, purpose: { kind: "usernameRegistration", username: "alice" } })).toBe("Username @alice");
    expect(txTitle({ ...lock, purpose: { kind: "usernameRegistration", username: "@alice" } })).toBe("Username @alice");
    expect(txTitle({ ...lock, label: "Rent", purpose: { kind: "usernameRegistration", username: "alice" } })).toBe("Rent");
    expect(txTitle(lock)).toBe("Dash Platform funding");
  });
});

describe("chips and icons", () => {
  it("is quiet when settled", () => {
    expect(txChip(status())).toBeNull();
    expect(txChip(status({ kind: "unconfirmed", chainLocked: false }))).toBe("Pending");
    expect(txChip(status({ kind: "unconfirmed", instantLocked: true, chainLocked: false }))).toBe("InstantSend");
    expect(txChip(status({ kind: "confirming", instantLocked: true, chainLocked: false }))).toBe("InstantSend");
    expect(txChip(status({ kind: "abandoned" }))).toBe("Abandoned");
    expect(txChip(status({ kind: "abandoned", dropped: { cause: "invalidSource", state: { kind: "releasing" } } }))).toBe("Dropped");
  });

  it("marks failures with the error icon", () => {
    expect(txIcon("sendToAddress", status({ kind: "conflicted" }), -5)).toBe("error");
    expect(txIcon("sendToSelf", status(), 0)).toBe("internalTransfer");
    expect(txIcon("recvWithAddress", status(), 5)).toBe("received");
  });

  it("shows a self-send fee as money that left", () => {
    expect(shownAmount(-429, "sendToSelf")).toEqual({ duffs: -429, signed: true });
    expect(shownAmount(-429, "platformTransfer")).toEqual({ duffs: 429, signed: false });
    expect(shownAmount(-429, "sendToAddress")).toEqual({ duffs: -429, signed: true });
  });

  it("groups adjacent mixing rows for list mode", () => {
    const row = (txid: string, txType: "coinJoinMixing" | "sendToAddress"): any => ({ txid, recordIndex: 0, txType, category: txType === "coinJoinMixing" ? "coinJoin" : "sent", amount: -1000, status: status(), timestamp: 1, blockHeight: 1, fee: 0, address: null, label: null, countsTowardBalance: true, involvesWatchOnly: false });
    const grouped = groupConsecutiveMixing([row("a", "coinJoinMixing"), row("b", "coinJoinMixing"), row("c", "sendToAddress"), row("d", "coinJoinMixing")]);
    expect(grouped).toHaveLength(3);
    expect("kind" in grouped[0] && grouped[0].rows).toHaveLength(2);
    expect(grouped[1]).toMatchObject({ txid: "c" });
    expect(grouped[2]).toMatchObject({ txid: "d" });
    expect("kind" in grouped[2]).toBe(false);
  });

  it("adds a group's amounts exactly, and gives no total past what a number holds exactly", () => {
    const row = (amount: number): any => ({ txid: "a", recordIndex: 0, txType: "recvWithCoinJoin", amount });
    expect(groupAmount([row(-1000), row(250), row(-1)])).toBe(-751);
    const big = 2 ** 52;
    expect(groupAmount([row(big), row(big - 1)])).toBe(Number.MAX_SAFE_INTEGER);
    expect(groupAmount([row(big), row(big)])).toBeNull();
    expect(groupAmount([row(-big), row(-big)])).toBeNull();
    expect(groupAmount([row(2 ** 53), row(-1)])).toBeNull();
  });

  it("keeps a conflicted or abandoned mixing row out of a group", () => {
    const row = (txid: string, kind: TxStatusInfo["kind"]): any => ({ txid, recordIndex: 0, txType: "coinJoinMixing", category: "coinJoin", amount: -1000, status: status({ kind }), timestamp: 1, blockHeight: 1, fee: 0, address: null, label: null, countsTowardBalance: true, involvesWatchOnly: false });
    const grouped = groupConsecutiveMixing([row("a", "confirmed"), row("b", "confirmed"), row("c", "conflicted"), row("d", "confirmed"), row("e", "abandoned"), row("f", "notAccepted")]);
    expect(grouped).toHaveLength(5);
    expect("kind" in grouped[0] && grouped[0].rows.map((r) => r.txid)).toEqual(["a", "b"]);
    expect(grouped[1]).toMatchObject({ txid: "c" });
    expect(grouped[2]).toMatchObject({ txid: "d" });
    expect(grouped[3]).toMatchObject({ txid: "e" });
    expect(grouped[4]).toMatchObject({ txid: "f" });
  });

  it("a collapsed group shows a member's Pending or InstantSend chip", () => {
    const row = (over: Partial<TxStatusInfo>): any => ({ txid: "a", recordIndex: 0, txType: "coinJoinMixing", status: status(over) });
    expect(groupChip([row({ kind: "confirmed" }), row({ kind: "confirmed" })])).toBeNull();
    expect(groupChip([row({ kind: "confirmed" }), row({ kind: "unconfirmed", instantLocked: true })])).toBe(txChip(status({ kind: "unconfirmed", instantLocked: true })));
    expect(groupChip([row({ kind: "unconfirmed", instantLocked: true }), row({ kind: "unconfirmed" })])).toBe(txChip(status({ kind: "unconfirmed" })));
  });
});

describe("days and addresses", () => {
  const now = new Date(2026, 9, 6, 12, 0);
  it("titles today and yesterday", () => {
    expect(dayTitle(new Date(2026, 9, 6, 1, 40), now, "en-US")).toBe("Today");
    expect(dayTitle(new Date(2026, 9, 5, 23, 59), now, "en-US")).toBe("Yesterday");
    expect(dayTitle(new Date(2026, 9, 4, 8, 0), now, "en-US")).toBe("October 4, 2026");
  });

  it("groups newest day first with undated items last", () => {
    const items = [new Date(2026, 9, 5, 9), null, new Date(2026, 9, 6, 1), new Date(2026, 9, 5, 8)];
    const groups = groupByDay(items, (d) => d);
    expect(groups.map((g) => g.items.length)).toEqual([1, 2, 1]);
    expect(groups[2].day).toBeNull();
  });

  it("shortens long addresses in the middle", () => {
    expect(shortAddress("yNBLvUCmZq3K8v4eusVmKavEj2XrPoQe1x")).toBe("yNBLvUCmZq3K…vEj2XrPoQe1x");
    expect(shortAddress("short")).toBe("short");
  });
});

describe("table mode", () => {
  it("uses dash-qt's type strings and party column", () => {
    expect(tableTypeTitle("recvWithAddress")).toBe("Received with");
    expect(tableTypeTitle("sendToOther")).toBe("Sent to");
    expect(tableTypeTitle("coinJoinMixing")).toBe("CoinJoin Mixing");
    expect(rowParty({ label: "Rent", address: "yAddr" })).toBe("Rent");
    expect(rowParty({ label: null, address: "yAddr" })).toBe("yAddr");
    expect(rowParty({ label: "", address: null })).toBe("(n/a)");
  });
});

describe("date filter", () => {
  // Tuesday 6 October 2026, noon (FIXTURE_NOW).
  const now = new Date(2026, 9, 6, 12, 0, 0);
  const s = (d: Date) => Math.floor(d.getTime() / 1000);

  it("bounds each choice in local time, weeks from Monday", () => {
    expect(dateRange("all", now)).toEqual({});
    expect(dateRange("today", now)).toEqual({ dateFrom: s(new Date(2026, 9, 6)) });
    expect(dateRange("thisWeek", now)).toEqual({ dateFrom: s(new Date(2026, 9, 5)) });
    expect(dateRange("thisMonth", now)).toEqual({ dateFrom: s(new Date(2026, 9, 1)) });
    expect(dateRange("lastMonth", now)).toEqual({ dateFrom: s(new Date(2026, 8, 1)), dateTo: s(new Date(2026, 9, 1)) });
    expect(dateRange("thisYear", now)).toEqual({ dateFrom: s(new Date(2026, 0, 1)) });
  });

  it("starts a Sunday's week on the Monday before", () => {
    expect(dateRange("thisWeek", new Date(2026, 9, 11, 9))).toEqual({ dateFrom: s(new Date(2026, 9, 5)) });
  });

  it("takes last December in January", () => {
    expect(dateRange("lastMonth", new Date(2027, 0, 15))).toEqual({ dateFrom: s(new Date(2026, 11, 1)), dateTo: s(new Date(2027, 0, 1)) });
  });
});
