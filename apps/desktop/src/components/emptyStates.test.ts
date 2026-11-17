import { describe, expect, it } from "vitest";
import { addressBookEmpty, activityFiltersApplied, EMPTY, historyEmptyCopy, walletListGate } from "./emptyStates";

describe("history empty", () => {
  it("stays quiet while the wallet is syncing, and offers Receive Dash once it has caught up", () => {
    const syncing = historyEmptyCopy({ syncing: true, home: false });
    expect(syncing).toMatchObject({
      art: "activity",
      title: "Your history is still syncing",
      message: "Payments show up here as the wallet catches up.",
      action: null,
    });
    expect(historyEmptyCopy({ syncing: true, home: true })).toBe(syncing);
    expect(historyEmptyCopy({ syncing: false, home: true }).action).toMatchObject({ label: "Receive Dash", testId: "history-empty-receive" });
    expect(historyEmptyCopy({ syncing: false, home: false }).action).toMatchObject({ label: "Receive Dash", testId: "tx-empty-receive" });
  });
});

describe("activityFiltersApplied", () => {
  const open = { categoryId: "all", text: "", date: "all" };

  it("is false for an untouched list", () => {
    expect(activityFiltersApplied(open)).toBe(false);
  });

  it("is true once a category, search, date or minimum amount is applied", () => {
    expect(activityFiltersApplied({ ...open, categoryId: "sent" })).toBe(true);
    expect(activityFiltersApplied({ ...open, text: "rent" })).toBe(true);
    expect(activityFiltersApplied({ ...open, date: "week" })).toBe(true);
    expect(activityFiltersApplied({ ...open, minAmount: 1 })).toBe(true);
  });
});

describe("addressBookEmpty", () => {
  it("is absent when there are rows", () => {
    expect(addressBookEmpty({ search: "", purpose: "send", count: 2 })).toBeNull();
  });

  it("offers add for sending, Receive Dash for receiving, and clear when a search misses", () => {
    expect(addressBookEmpty({ search: "", purpose: "send", count: 0 })).toMatchObject({ step: "add", action: { label: "Add address" } });
    expect(addressBookEmpty({ search: "", purpose: "receive", count: 0 })).toMatchObject({ step: "receive", action: { label: "Receive Dash" } });
    expect(addressBookEmpty({ search: "zz", purpose: "receive", count: 0 })).toMatchObject({ step: "clear", art: "search" });
  });
});

describe("wallet list", () => {
  it("offers a wallet only after the list has loaded empty", () => {
    expect(walletListGate({ isPending: true, isError: false })).toBe("loading");
    expect(walletListGate({ isPending: false, isError: true })).toBe("error");
    expect(walletListGate({ isPending: false, isError: false, data: [] })).toBe("empty");
    expect(walletListGate({ isPending: false, isError: false, data: [1, 2] })).toBe("ready");
  });

  it("keeps the wallets it had when a refetch fails", () => {
    expect(walletListGate({ isPending: false, isError: true, data: [1] })).toBe("ready");
    expect(walletListGate({ isPending: false, isError: true, data: [] })).toBe("empty");
  });
});

describe("empty copy", () => {
  it("names the inbox, the coin list and the mixed-coin list, and leaves peers and the no-wallet page without a button", () => {
    expect(EMPTY.notifications).toMatchObject({
      title: "No notifications yet",
      message: "Contact requests and payments from your contacts show up here.",
      action: { label: "Add contact" },
    });
    expect(EMPTY.notificationsNoIdentity.action?.label).toBe("Join DashPay");
    expect(EMPTY.coins.title).toBe("No coins yet");
    expect(EMPTY.coins.message.endsWith("show up here.")).toBe(true);
    expect(EMPTY.coinsHidden).toMatchObject({
      title: "Every coin here is a CoinJoin coin",
      message: "CoinJoin coins are hidden from this list.",
      action: { label: "Show all coins" },
    });
    expect(EMPTY.peers).toMatchObject({
      message: "Check your internet connection. The wallet keeps looking for peers.",
      action: null,
    });
    expect(EMPTY.wallet).toMatchObject({ message: "Create or restore a wallet to get started.", action: null });
    expect(EMPTY.contacts.action?.label).toBe("Add contact");
  });

  it("does not label a button Add a contact", () => {
    for (const copy of Object.values(EMPTY)) expect(copy.action?.label).not.toBe("Add a contact");
  });
});
