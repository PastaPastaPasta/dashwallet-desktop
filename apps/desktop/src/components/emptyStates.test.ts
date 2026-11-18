import { describe, expect, it } from "vitest";
import { addressBookEmpty, activityFiltersApplied, coinsEmptyCopy, EMPTY, historyEmptyCopy, listArrival, walletListGate, walletReadyToOpen, type ListArrival } from "./emptyStates";

const reading = (over: Partial<{ running: boolean; caughtUp: boolean; tipHeight: number | null }> = {}) => ({
  running: false,
  caughtUp: false,
  tipHeight: null as number | null,
  ...over,
});

describe("list arrival", () => {
  it("names every sync state, and treats an empty list as settled only once caught up", () => {
    expect(listArrival({ failed: false })).toBe("starting");
    expect(listArrival({ status: null, failed: false })).toBe("starting");
    expect(listArrival({ status: reading(), failed: false })).toBe("starting");
    expect(listArrival({ status: reading({ running: true }), failed: false })).toBe("syncing");
    expect(listArrival({ status: reading({ running: true, tipHeight: 10 }), failed: false })).toBe("syncing");
    expect(listArrival({ status: reading({ tipHeight: 10 }), failed: false })).toBe("stopped");
    expect(listArrival({ failed: true })).toBe("unreadable");
    expect(listArrival({ status: null, failed: true })).toBe("unreadable");
    expect(listArrival({ status: reading({ running: true, caughtUp: true, tipHeight: 10 }), failed: false })).toBe("settled");
    // A later failed read keeps the caught-up reading it already had.
    expect(listArrival({ status: reading({ caughtUp: true, tipHeight: 10 }), failed: true })).toBe("settled");
  });
});

describe("history empty", () => {
  it("stays quiet until the wallet has caught up, and offers Receive Dash only then", () => {
    const syncing = historyEmptyCopy({ arrival: "syncing", home: false });
    expect(syncing).toMatchObject({
      art: "activity",
      title: "Your history is still syncing",
      message: "Payments show up here as the wallet catches up.",
      action: null,
    });
    expect(historyEmptyCopy({ arrival: "syncing", home: true })).toBe(syncing);
    const waiting: ListArrival[] = ["starting", "syncing", "stopped", "unreadable"];
    for (const arrival of waiting) {
      expect(historyEmptyCopy({ arrival, home: true }).action).toBeNull();
      expect(historyEmptyCopy({ arrival, home: false }).action).toBeNull();
    }
    expect(new Set(waiting.map((arrival) => historyEmptyCopy({ arrival, home: false }).title)).size).toBe(waiting.length);
    expect(historyEmptyCopy({ arrival: "starting", home: true }).title).toBe("The wallet is still starting");
    expect(historyEmptyCopy({ arrival: "stopped", home: false }).title).toBe("Syncing has stopped");
    expect(historyEmptyCopy({ arrival: "unreadable", home: false }).title).toBe("Could not tell whether the wallet has caught up");
    expect(historyEmptyCopy({ arrival: "settled", home: true }).action).toMatchObject({ label: "Receive Dash", testId: "history-empty-receive" });
    expect(historyEmptyCopy({ arrival: "settled", home: false }).action).toMatchObject({ label: "Receive Dash", testId: "tx-empty-receive" });
  });
});

describe("coins empty", () => {
  it("offers Receive Dash only once caught up, and names each earlier state", () => {
    const waiting: ListArrival[] = ["starting", "syncing", "stopped", "unreadable"];
    for (const arrival of waiting) expect(coinsEmptyCopy(arrival).action).toBeNull();
    expect(new Set(waiting.map((arrival) => coinsEmptyCopy(arrival).title)).size).toBe(waiting.length);
    expect(coinsEmptyCopy("syncing")).toMatchObject({
      title: "Coins are still syncing",
      message: "Coins you already have show up here as the wallet catches up.",
      action: null,
    });
    expect(coinsEmptyCopy("starting").title).toBe("The wallet is still starting");
    expect(coinsEmptyCopy("stopped").title).toBe("Syncing has stopped");
    expect(coinsEmptyCopy("unreadable").title).toBe("Could not tell whether the wallet has caught up");
    expect(coinsEmptyCopy("settled").action).toMatchObject({ label: "Receive Dash", testId: "coins-empty-receive" });
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

  it("opens after a save only once the list it read contains that wallet", () => {
    const saved = "aa";
    expect(walletReadyToOpen(saved, undefined)).toBe(false);
    expect(walletReadyToOpen(saved, [])).toBe(false);
    expect(walletReadyToOpen(saved, [{ walletId: "bb" }])).toBe(false);
    expect(walletReadyToOpen(saved, [{ walletId: "bb" }, { walletId: saved }])).toBe(true);
    expect(walletReadyToOpen(null, undefined)).toBe(false);
    expect(walletReadyToOpen(null, [])).toBe(false);
    expect(walletReadyToOpen(null, [{ walletId: "bb" }])).toBe(true);
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
