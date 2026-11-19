import { describe, expect, it } from "vitest";
import { addressBookEmpty, activityFiltersApplied, catchUpArrival, coinsEmptyCopy, contactsDisplay, contactsHeaderAdd, EMPTY, historyEmptyCopy, listArrival, listDisplay, listShowsCount, optionalHistorySource, sourceGap, walletListGate, walletReadyToOpen, type ListArrival } from "./emptyStates";

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

describe("catch-up for one wallet", () => {
  const settled = { status: reading({ running: true, caughtUp: true, tipHeight: 100 }), failed: false };

  it("uses the raw network reading, then this wallet's scan height", () => {
    expect(catchUpArrival({ ...settled, scanPending: false, scanFailed: false, scanHeight: 100 })).toBe("settled");
    expect(catchUpArrival({ ...settled, scanPending: false, scanFailed: false, scanHeight: 101 })).toBe("settled");
    expect(catchUpArrival({ ...settled, scanPending: false, scanFailed: false, scanHeight: 99 })).toBe("syncing");
    expect(catchUpArrival({ ...settled, scanPending: false, scanFailed: false, scanHeight: null })).toBe("starting");
    expect(catchUpArrival({ ...settled, scanPending: true, scanFailed: false, scanHeight: undefined })).toBe("starting");
    expect(catchUpArrival({ ...settled, scanPending: false, scanFailed: true, scanHeight: undefined })).toBe("unreadable");
    // The network not being caught up wins, even when the scan height looks finished.
    expect(catchUpArrival({ status: reading({ running: true, tipHeight: 100 }), failed: false, scanPending: false, scanFailed: false, scanHeight: 100 })).toBe("syncing");
    expect(catchUpArrival({ status: reading({ tipHeight: 100 }), failed: false, scanPending: false, scanFailed: false, scanHeight: 100 })).toBe("stopped");
  });
});

describe("list display", () => {
  const base = { rowCount: 0, pending: false, failed: false, hasSnapshot: true, arrival: "settled" as ListArrival };
  const extra = { pending: false, failed: false, hasSnapshot: true, rowCount: 0 };

  it("keeps rows, waits while anything is still out, and confirms empty only then", () => {
    expect(listDisplay({ ...base, rowCount: 3 })).toEqual({ kind: "rows", incomplete: false });
    expect(listDisplay({ ...base, pending: true, hasSnapshot: false })).toEqual({ kind: "arriving", why: "pending" });
    expect(listDisplay({ ...base, failed: true, hasSnapshot: false })).toEqual({ kind: "failed" });
    expect(listDisplay({ ...base, arrival: "syncing" })).toEqual({ kind: "arriving", why: "syncing" });
    expect(listDisplay({ ...base, arrival: "starting" })).toEqual({ kind: "arriving", why: "starting" });
    expect(listDisplay({ ...base, arrival: "stopped" })).toEqual({ kind: "arriving", why: "stopped" });
    expect(listDisplay({ ...base, arrival: "unreadable" })).toEqual({ kind: "arriving", why: "unreadable" });
    expect(listDisplay({ ...base, extras: [{ ...extra, pending: true, hasSnapshot: false }] })).toEqual({ kind: "arriving", why: "syncing" });
    expect(listDisplay({ ...base, arrival: "stopped", extras: [{ ...extra, pending: true, hasSnapshot: false }] })).toEqual({ kind: "arriving", why: "stopped" });
    expect(listDisplay(base)).toEqual({ kind: "empty" });
  });

  it("does not call an empty snapshot empty once a later read of it has failed", () => {
    // The read that failed may be the one that had the rows: retry, not Receive.
    expect(listDisplay({ ...base, failed: true })).toEqual({ kind: "failed" });
    expect(listDisplay({ ...base, extras: [{ ...extra, failed: true }] })).toEqual({ kind: "failed" });
    expect(listShowsCount(listDisplay({ ...base, failed: true }), true)).toBe(false);
    // Rows in hand stay, with the failed source beside them.
    expect(listDisplay({ ...base, rowCount: 2, extras: [{ ...extra, failed: true }] })).toEqual({ kind: "rows", incomplete: true });
    expect(listDisplay({ ...base, failed: true, extras: [{ ...extra, rowCount: 1 }] })).toEqual({ kind: "rows", incomplete: true });
    // A failed refetch of a source that has rows keeps them, but they are not a count to show.
    expect(listDisplay({ ...base, rowCount: 2, failed: true })).toEqual({ kind: "rows", incomplete: true });
    expect(listDisplay({ ...base, rowCount: 2, extras: [{ ...extra, rowCount: 1, failed: true }] })).toEqual({ kind: "rows", incomplete: true });
    expect(sourceGap({ ...extra, failed: true, rowCount: 3 })).toBe(false);
  });

  it("does not count rows found while the wallet is still catching up", () => {
    // A restore that has found its first payment is still scanning for the rest.
    for (const arrival of ["starting", "syncing", "stopped", "unreadable"] as const) {
      const display = listDisplay({ ...base, rowCount: 1, arrival });
      expect(display).toEqual({ kind: "rows", incomplete: true });
      expect(listShowsCount(display, false)).toBe(false);
    }
    expect(listShowsCount(listDisplay({ ...base, rowCount: 1 }), false)).toBe(true);
  });

  it("does not offer an empty snapshot that is being read again as empty", () => {
    // A payment arrived: history and coins are stale until their reads are back.
    expect(listDisplay({ ...base, refreshing: true })).toEqual({ kind: "arriving", why: "pending" });
    expect(listDisplay({ ...base, extras: [{ ...extra, refreshing: true }] })).toEqual({ kind: "arriving", why: "pending" });
    expect(listShowsCount(listDisplay({ ...base, refreshing: true }), true)).toBe(false);
    // While catching up the syncing card stays as it is, and a list with rows does not flicker.
    expect(listDisplay({ ...base, refreshing: true, arrival: "syncing" })).toEqual({ kind: "arriving", why: "syncing" });
    expect(listDisplay({ ...base, rowCount: 2, refreshing: true })).toEqual({ kind: "rows", incomplete: false });
    expect(listDisplay({ ...base, refreshing: false })).toEqual({ kind: "empty" });
  });

  it("does not call a rejected optional read empty, and keeps a failed first read beside usable rows", () => {
    const rejected = { pending: false, failed: true, hasSnapshot: false, rowCount: 0 };
    expect(listDisplay({ ...base, extras: [rejected] })).toEqual({ kind: "failed" });
    expect(listDisplay({ ...base, extras: [rejected, { ...extra, rowCount: 2 }] })).toEqual({ kind: "rows", incomplete: true });
    expect(listDisplay({ ...base, rowCount: 0, failed: true, hasSnapshot: false, extras: [{ ...extra, rowCount: 1 }] })).toEqual({ kind: "rows", incomplete: true });
    // A missing command is not passed in, so it neither fails the list nor holds it open.
    expect(listDisplay({ ...base, extras: [] })).toEqual({ kind: "empty" });
  });

  it("omits absent commands and unsupported optional pools, and flags real read failures", () => {
    expect(optionalHistorySource({ isPending: false, isError: false, isSuccess: true, isFetching: false, data: null }, 0)).toBeNull();
    for (const code of ["shielded.not_active", "platform.feature_off", "not_implemented", "platform.not_implemented"]) {
      expect(
        optionalHistorySource(
          { isPending: false, isError: true, isSuccess: false, isFetching: false, data: undefined, error: { code, message: code, data: null } },
          0,
        ),
      ).toBeNull();
    }
    expect(
      optionalHistorySource(
        { isPending: false, isError: true, isSuccess: false, isFetching: false, data: undefined, error: { code: "internal", message: "boom", data: null } },
        0,
      ),
    ).toEqual({ pending: false, failed: true, hasSnapshot: false, rowCount: 0, refreshing: false });
    expect(optionalHistorySource({ isPending: true, isError: false, isSuccess: false, isFetching: true, data: undefined }, 0)).toEqual({
      pending: true,
      failed: false,
      hasSnapshot: false,
      rowCount: 0,
      refreshing: true,
    });
    expect(optionalHistorySource({ isPending: false, isError: false, isSuccess: true, isFetching: true, data: [{ id: "1" }] }, 1)).toEqual({
      pending: false,
      failed: false,
      hasSnapshot: true,
      rowCount: 1,
      refreshing: true,
    });
  });

  it("counts zero only for a complete row list, or a filter on a confirmed empty list", () => {
    expect(listShowsCount({ kind: "rows", incomplete: false }, false)).toBe(true);
    expect(listShowsCount({ kind: "rows", incomplete: true }, false)).toBe(false);
    expect(listShowsCount({ kind: "empty" }, true)).toBe(true);
    expect(listShowsCount({ kind: "empty" }, false)).toBe(false);
    expect(listShowsCount({ kind: "arriving", why: "syncing" }, true)).toBe(false);
    expect(listShowsCount({ kind: "failed" }, true)).toBe(false);
  });
});

describe("contacts display", () => {
  const base = { pending: false, failed: false, hasSnapshot: true, rowCount: 0, updating: false, searching: false };

  it("hides Add contact only when the empty card that replaces it is showing", () => {
    const empty = contactsDisplay(base);
    expect(empty).toEqual({ kind: "empty" });
    expect(contactsHeaderAdd(empty)).toBe(false);
    const updating = contactsDisplay({ ...base, updating: true });
    expect(updating).toEqual({ kind: "arriving" });
    expect(contactsHeaderAdd(updating)).toBe(true);
    expect(contactsHeaderAdd(contactsDisplay({ ...base, rowCount: 2, updating: true }))).toBe(true);
    expect(contactsHeaderAdd(contactsDisplay({ ...base, searching: true }))).toBe(true);
    expect(contactsHeaderAdd(contactsDisplay({ ...base, pending: true, hasSnapshot: false }))).toBe(true);
    expect(contactsHeaderAdd(contactsDisplay({ ...base, failed: true, hasSnapshot: false }))).toBe(true);
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
    for (const arrival of ["starting", "syncing", "stopped"] as const) {
      expect(historyEmptyCopy({ arrival, home: true }).action).toBeNull();
      expect(historyEmptyCopy({ arrival, home: false }).action).toBeNull();
    }
    // A catch-up that could not be read is read again, never Receive.
    expect(historyEmptyCopy({ arrival: "unreadable", home: true }).action).toEqual({ label: "Try again", testId: "history-recheck" });
    expect(historyEmptyCopy({ arrival: "unreadable", home: false }).action).toEqual({ label: "Try again", testId: "tx-recheck" });
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
    for (const arrival of ["starting", "syncing", "stopped"] as const) expect(coinsEmptyCopy(arrival).action).toBeNull();
    expect(coinsEmptyCopy("unreadable").action).toEqual({ label: "Try again", testId: "coins-recheck" });
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
