import { describe, expect, it } from "vitest";
import type { TxStatusInfo } from "../api/generated/TxStatusInfo";
import { contactLetters, contactName, dateRange, dayTitle, groupByDay, rowParty, shortAddress, shownAmount, tableTypeTitle, txChip, txIcon, txTitle, typeTitle } from "./tx";

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

  it("maps dash-qt types to iOS titles", () => {
    expect(typeTitle("coinJoinMixing", "coinJoin", 0)).toBe("Mixing");
    expect(typeTitle("recvWithAddress", "reward", 5)).toBe("Masternode reward");
    expect(typeTitle("generated", "reward", 5)).toBe("Mined");
    expect(typeTitle("other", "other", -1)).toBe("Sent");
  });
});

describe("chips and icons", () => {
  it("is quiet when settled", () => {
    expect(txChip(status())).toBeNull();
    expect(txChip(status({ kind: "unconfirmed", chainLocked: false }))).toBe("Pending");
    expect(txChip(status({ kind: "unconfirmed", instantLocked: true, chainLocked: false }))).toBe("InstantSend");
    expect(txChip(status({ kind: "confirming", instantLocked: true, chainLocked: false }))).toBe("InstantSend");
    expect(txChip(status({ kind: "abandoned" }))).toBe("Abandoned");
  });

  it("marks failures with the error icon", () => {
    expect(txIcon("sendToAddress", status({ kind: "conflicted" }), -5)).toBe("error");
    expect(txIcon("sendToSelf", status(), 0)).toBe("internalTransfer");
    expect(txIcon("recvWithAddress", status(), 5)).toBe("received");
  });

  it("shows internal moves without a sign", () => {
    expect(shownAmount(-429, "sendToSelf")).toEqual({ duffs: 429, signed: false });
    expect(shownAmount(-429, "sendToAddress")).toEqual({ duffs: -429, signed: true });
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
