import { QueryClient, QueryObserver } from "@tanstack/react-query";
import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { InfiniteData } from "@tanstack/react-query";
import { bridgeEvents, invalidationsFor, ofOtherNetwork, RECONCILE_COALESCE_MS, RECONCILE_MAX_WAIT_MS, toAnnounce } from "./events";
import type { Backend } from "./backend";
import type { AppEvent } from "./generated/commands";
import type { HistoryRows } from "./generated/HistoryRows";
import type { LockOutcome } from "./generated/LockOutcome";
import type { LockStatus } from "./generated/LockStatus";
import type { TxNoticeRow } from "./generated/TxNoticeRow";
import { onReceived, receivedNow, resetReceived } from "../payments/received";
import { installFixture } from "../fixture";
import { FIXTURE_WALLET_ID } from "../fixture/scenarios";
import { profileKeys } from "../dashpay/profileQueries";
import { dashpayKeys } from "../dashpay/queries";
import { mnKeys } from "../masternodes/queries";
import { lockGeneration, lockUnconfirmed, resetWalletGate, seedLockStatus, walletGateClosed } from "./lockGate";
import { keys } from "./queries";
import { IpcBackend } from "./tauri";

const LOCK: LockStatus = { state: "locked", encrypted: true, quickUnlockEnrolled: false, failedAttempts: 0, retryAfterSecs: null, walletsWithSecrets: [] };
const NOTICE: TxNoticeRow = { txid: "", recordIndex: 0, amount: 5000, timestamp: null, txType: "recvWithAddress", address: null, label: null, coinjoinInternal: false };

const REPORT: LockOutcome = {
  status: { state: "locked", encrypted: true, quickUnlockEnrolled: false, failedAttempts: 0, retryAfterSecs: null, walletsWithSecrets: [] },
  flows: [],
  flowsOmitted: 0,
  walletsWithSecretsOmitted: 0,
};

/** Every bridge a test opens is closed after it: each follows the one window-wide lock gate. */
const bridges: (() => void)[] = [];
const bridged = async (...args: Parameters<typeof bridgeEvents>) => {
  const stop = await bridgeEvents(...args);
  bridges.push(stop);
  return stop;
};

beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  resetWalletGate();
});
afterEach(() => {
  bridges.splice(0).forEach((stop) => stop());
  clearMocks();
  vi.unstubAllGlobals();
  resetWalletGate();
});

describe("event bridge", () => {
  it("maps each signal to the queries it touches", () => {
    expect(invalidationsFor({ kind: "historyChanged", network: "testnet", walletId: "w", txids: [] })).toEqual([
      ["history", "w"],
      ["txDetail", "w"],
      ["droppedNotices", "w"],
      ["receive", "w"],
      ["coins", "w"],
      ["dashpay", "contacts", "w"],
      ["masternodes", "payouts", "w"],
      ["masternodes", "sharedSessions", "w"],
      ["masternodes", "sharedSession", "w"],
      ["masternodes", "sharedUnreadable", "w"],
    ]);
    expect(invalidationsFor({ kind: "balances", network: "testnet", walletId: "w", balances: null })).toEqual([
      keys.balance("w"),
      keys.wallets,
      keys.coinsAll("w"),
      dashpayKeys.quote("w"),
      keys.coinjoinStatus("w"),
      mnKeys.list("w"),
      keys.droppedNotices("w"),
      keys.txDetailAll("w"),
    ]);
    // A lock drops the wallet's queries rather than reading them again; an unlock reads them.
    expect(invalidationsFor({ kind: "vaultLockState", network: "testnet", state: "locked" })).toEqual([keys.lockStatus, keys.lockReconcile]);
    expect(invalidationsFor({ kind: "vaultLockState", network: "testnet", state: "unlocked" })).toEqual([keys.lockStatus, keys.wallets, keys.lockReconcile, dashpayKeys.registrationsAll, mnKeys.listAll, mnKeys.detailAll, keys.shieldedAll]);
    expect(invalidationsFor({ kind: "leaseChanged", network: "testnet", lease: {} as never })).toEqual([keys.lockReconcile]);
    // DP1-09 D14: a registration lease's change re-reads the rows, whose `waiting` moved.
    const registration = { flow: "registration" } as never;
    expect(invalidationsFor({ kind: "leaseChanged", network: "testnet", lease: registration })).toEqual([keys.lockReconcile, dashpayKeys.registrationsAll]);
    // A settled funding step re-reads the stuck top-up list (DP1-10 D3).
    expect(invalidationsFor({ kind: "dispatchResolved", network: "testnet", settled: {} as never })).toEqual([keys.lockReconcile, profileKeys.stuckTopUpsAll]);
    expect(invalidationsFor({ kind: "sessionOpened", network: "testnet" })).toBe("all");
    // Every governance read, whichever changed (BL-232); every engine-local masternode read (MG-10 §3.5).
    expect(invalidationsFor({ kind: "governance", network: "testnet" })).toEqual([keys.governanceAll]);
    // Issue 340: a failed automatic backup rereads Settings ▸ Backups; other notices read nothing.
    expect(invalidationsFor({ kind: "notice", network: "testnet", code: "backupFailed", detail: "" })).toEqual([keys.backupsAll]);
    expect(invalidationsFor({ kind: "notice", network: "testnet", code: "spvError", detail: "" })).toEqual([]);
    // A written automatic backup rereads them too, so an open tab drops a failure row it cleared.
    expect(invalidationsFor({ kind: "backups", network: "testnet" })).toEqual([keys.backupsAll]);
    expect(invalidationsFor({ kind: "masternodes", network: "testnet" })).toEqual([["masternodes"]]);
    expect(invalidationsFor({ kind: "eventsOverflowed" })).toBe("all");
    expect(invalidationsFor({ kind: "lockProgress", network: "testnet", progress: { kind: "draining", inFlight: 1, deadlineInMs: 500 } })).toEqual([]);
    expect(invalidationsFor({ kind: "platform", network: "testnet", walletId: "w", change: { domain: "names" } })).toEqual([keys.dashpayAll, keys.historyAll("w"), keys.txDetailAll("w"), keys.receiveAll("w")]);
    // The lock's "Username @name" title follows the registration (PRODUCT #2).
    expect(invalidationsFor({ kind: "platform", network: "testnet", walletId: "w", change: { domain: "registration", draft: "d" } })).toEqual([
      keys.dashpayAll,
      keys.historyAll("w"),
      keys.txDetailAll("w"),
    ]);
    expect(invalidationsFor({ kind: "platform", network: "testnet", walletId: "w", change: { domain: "identities" } })).toEqual([
      keys.dashpayAll,
      keys.historyAll("w"),
      keys.txDetailAll("w"),
      keys.receiveAll("w"),
    ]);
    for (const change of [{ domain: "contacts", identity: "i" }, { domain: "payments", identity: "i" }] as const)
      expect(invalidationsFor({ kind: "platform", network: "testnet", walletId: "w", change })).toEqual([keys.dashpayAll, keys.historyAll("w"), keys.txDetailAll("w")]);
    expect(invalidationsFor({ kind: "platform", network: "testnet", walletId: "w", change: { domain: "events", identity: "i" } })).toEqual([keys.dashpayAll]);
    // The bring-up's outcome, and sync, move the DashPay status (BL-153 D8, D9).
    expect(invalidationsFor({ kind: "platform", network: "testnet", walletId: "w", change: { domain: "startup" } })).toEqual([keys.dashpayAll]);
    expect(invalidationsFor({ kind: "spvStateChanged", network: "testnet", running: true })).toEqual([keys.sync, keys.scanHeightAll, keys.dashpayStatusAll, dashpayKeys.syncInfoAll, keys.unfinishedTransfersAll, keys.protocolCompatibility, mnKeys.listState]);
    expect(invalidationsFor({ kind: "sync", network: "testnet", status: {} as never })).toEqual([keys.sync, keys.scanHeightAll, keys.dashpayStatusAll, dashpayKeys.syncInfoAll, keys.unfinishedTransfersAll, mnKeys.listState, keys.restored, keys.restoreStatus]);
    // U-06: the engine's CoinJoin signal, and a balance change (D7), re-read the wallet's mixing status.
    expect(invalidationsFor({ kind: "coinJoin", network: "testnet", walletId: "w" })).toEqual([keys.coinjoinStatus("w"), keys.coinsAll("w")]);
    const report = REPORT;
    expect(invalidationsFor({ kind: "lockProgress", network: "testnet", progress: { kind: "done", report } })).toEqual([keys.lockStatus, keys.lockReconcile]);
  });

  it("shows a lock's report with its done, and coalesces reconciliations", async () => {
    vi.useFakeTimers();
    try {
      const client = new QueryClient();
      const previous = { ...REPORT, flowsOmitted: 1 };
      client.setQueryData(keys.lastLockReport, previous);
      client.setQueryData([...keys.lockReconcile, previous], { settled: false });
      let emit: (e: AppEvent) => void = () => undefined;
      const fake = { onEvent: async (l: (e: AppEvent) => void) => ((emit = l), () => undefined) } as unknown as Backend;
      await bridged(fake, client, "testnet");
      emit({ kind: "lockProgress", network: "testnet", progress: { kind: "draining", inFlight: 1, deadlineInMs: 500 } });
      expect(client.getQueryData(keys.lockDrain)).toMatchObject({ inFlight: 1 });
      emit({ kind: "lockProgress", network: "testnet", progress: { kind: "done", report: REPORT } });
      expect(client.getQueryData(keys.lastLockReport)).toEqual(REPORT);
      expect(client.getQueryData(keys.lockDrain)).toBeNull();

      const reconcile = () => client.getQueryState([...keys.lockReconcile, previous])?.isInvalidated;
      for (let i = 0; i < 16; i++) emit({ kind: "leaseChanged", network: "testnet", lease: {} as never });
      expect(reconcile()).toBe(false);
      await vi.advanceTimersByTimeAsync(RECONCILE_COALESCE_MS);
      expect(reconcile()).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  // A `lastLockReport` reply the app sent before it kept the new report, arriving after `done`.
  it("a slow lastLockReport reply cannot replace the report done brought", async () => {
    const fixture = installFixture({ scenario: "lock-report-resolved" });
    const backend = new IpcBackend("fixture");
    const client = new QueryClient();
    await bridged(backend, client, "testnet");
    const previous = await backend.lock();
    await vi.waitFor(() => expect(client.getQueryData(keys.lastLockReport)).toEqual(previous));

    const release = fixture.hold("last_lock_report");
    const slow = client.fetchQuery({ queryKey: keys.lastLockReport, queryFn: () => backend.lastLockReport(), staleTime: 0 }).catch(() => null);
    await vi.waitFor(() => expect(fixture.calls.at(-1)).toBe("last_lock_report"));
    const newer: LockOutcome = { ...previous, flowsOmitted: 7 };
    fixture.world.lastLockReport = newer;
    fixture.emit({ kind: "lockProgress", network: "testnet", progress: { kind: "done", report: newer } });
    await vi.waitFor(() => expect(client.getQueryData(keys.lastLockReport)).toEqual(newer));

    release();
    await slow;
    await new Promise((r) => setTimeout(r, 0));
    expect(client.getQueryData(keys.lastLockReport)).toEqual(newer);
  });

  it("a steady stream of events still reconciles, and unsubscribing drops a pending one", async () => {
    vi.useFakeTimers();
    try {
      const client = new QueryClient();
      const key = [...keys.lockReconcile, REPORT];
      client.setQueryData(key, { settled: false });
      let emit: (e: AppEvent) => void = () => undefined;
      const fake = { onEvent: async (l: (e: AppEvent) => void) => ((emit = l), () => undefined) } as unknown as Backend;
      const unsubscribe = await bridged(fake, client, "testnet");
      const lease = () => emit({ kind: "leaseChanged", network: "testnet", lease: {} as never });
      for (let t = 0; t < RECONCILE_MAX_WAIT_MS; t += RECONCILE_COALESCE_MS / 2) {
        lease();
        await vi.advanceTimersByTimeAsync(RECONCILE_COALESCE_MS / 2);
      }
      expect(client.getQueryState(key)?.isInvalidated).toBe(true);

      client.setQueryData(key, { settled: false });
      lease();
      unsubscribe();
      await vi.advanceTimersByTimeAsync(RECONCILE_MAX_WAIT_MS);
      expect(client.getQueryState(key)?.isInvalidated).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it("invalidates the lock status when the fixture vault locks, and drops the wallet's reads", async () => {
    installFixture({ scenario: "funded" });
    const backend = new IpcBackend("fixture");
    const client = new QueryClient();
    client.setQueryData(keys.lockStatus, await backend.lockStatus());
    client.setQueryData(keys.balance(FIXTURE_WALLET_ID), await backend.balance(FIXTURE_WALLET_ID));
    await bridged(backend, client, "testnet");
    await backend.lock();
    expect(client.getQueryState(keys.lockStatus)?.isInvalidated).toBe(true);
    expect(walletGateClosed()).toBe(true);
    expect(client.getQueryState(keys.balance(FIXTURE_WALLET_ID))).toBeUndefined();
  });

  it("cuts a loaded history list back to its first page before refetching", async () => {
    const client = new QueryClient();
    const key = keys.history(FIXTURE_WALLET_ID, { categories: [], text: null, sort: "newestFirst" }, 2);
    const page = { rows: [], nextCursor: "x", totalMatching: 4 };
    client.setQueryData<InfiniteData<HistoryRows, string | null>>(key, { pages: [page, page], pageParams: [null, "2"] });
    let emit: (e: AppEvent) => void = () => undefined;
    const fake = { onEvent: async (l: (e: AppEvent) => void) => ((emit = l), () => undefined) } as unknown as Backend;
    await bridged(fake, client, "testnet");
    emit({ kind: "historyChanged", network: "testnet", walletId: FIXTURE_WALLET_ID, txids: [] });
    expect(client.getQueryData<InfiniteData<HistoryRows, string | null>>(key)?.pages).toHaveLength(1);
    expect(client.getQueryState(key)?.isInvalidated).toBe(true);
  });

  it("a seed attached to a watch-only wallet re-reads its DashPay status (BL-153 D19)", async () => {
    const client = new QueryClient();
    client.setQueryData(keys.dashpayStatus("a"), { kind: "noIdentity", reason: "watchOnly" });
    client.setQueryData(keys.dashpayStatus("b"), { kind: "noIdentity", reason: "watchOnly" });
    let emit: (e: AppEvent) => void = () => undefined;
    const fake = { onEvent: async (l: (e: AppEvent) => void) => ((emit = l), () => undefined) } as unknown as Backend;
    await bridged(fake, client, "testnet");
    emit({ kind: "walletCreated", network: "testnet", walletId: "a" });
    expect(client.getQueryState(keys.dashpayStatus("a"))?.isInvalidated).toBe(true);
    expect(client.getQueryState(keys.dashpayStatus("b"))?.isInvalidated).toBe(false);
  });

  it("drops a wallet's scan height when that wallet comes, goes, or the network changes, without a sync event", async () => {
    const client = new QueryClient();
    client.setQueryData(keys.scanHeight("a"), 100);
    client.setQueryData(keys.scanHeight("b"), 80);
    let emit: (e: AppEvent) => void = () => undefined;
    const fake = { onEvent: async (l: (e: AppEvent) => void) => ((emit = l), () => undefined) } as unknown as Backend;
    await bridged(fake, client, "testnet");
    emit({ kind: "walletRemoved", network: "testnet", walletId: "a" });
    expect(client.getQueryData(keys.scanHeight("a"))).toBeUndefined();
    expect(client.getQueryData(keys.scanHeight("b"))).toBe(80);
    client.setQueryData(keys.scanHeight("a"), 100);
    emit({ kind: "walletCreated", network: "testnet", walletId: "a" });
    expect(client.getQueryData(keys.scanHeight("a"))).toBeUndefined();
    client.setQueryData(keys.scanHeight("b"), 80);
    emit({ kind: "walletLoadChanged", network: "testnet", walletId: "b", loaded: true });
    expect(client.getQueryData(keys.scanHeight("b"))).toBeUndefined();
    client.setQueryData(keys.scanHeight("a"), 50);
    emit({ kind: "sessionOpened", network: "testnet" });
    expect(client.getQueryData(keys.scanHeight("a"))).toBeUndefined();
    client.setQueryData(keys.scanHeight("a"), 50);
    emit({ kind: "sessionClosed", network: "testnet" });
    expect(client.getQueryData(keys.scanHeight("a"))).toBeUndefined();
    client.setQueryData(keys.scanHeight("a"), 50);
    emit({ kind: "eventsOverflowed" });
    expect(client.getQueryData(keys.scanHeight("a"))).toBeUndefined();
  });

  it("a wallet that comes or goes drops its cached receive reads (DP3-03b D17)", async () => {
    const client = new QueryClient();
    for (const w of ["a", "b"]) {
      client.setQueryData(keys.usernameInRequests(w), true);
      client.setQueryData(keys.receiveUsername(w), "alice");
    }
    let emit: (e: AppEvent) => void = () => undefined;
    const fake = { onEvent: async (l: (e: AppEvent) => void) => ((emit = l), () => undefined) } as unknown as Backend;
    await bridged(fake, client, "testnet");
    emit({ kind: "walletCreated", network: "testnet", walletId: "a" });
    expect(client.getQueryData(keys.usernameInRequests("a"))).toBeUndefined();
    expect(client.getQueryData(keys.receiveUsername("a"))).toBeUndefined();
    expect(client.getQueryData(keys.usernameInRequests("b"))).toBe(true);
    emit({ kind: "walletRemoved", network: "testnet", walletId: "b" });
    expect(client.getQueryData(keys.receiveUsername("b"))).toBeUndefined();
  });

  it("sync re-reads only the DashPay statuses without an identity (BL-153 D9, DP1-09 D28)", async () => {
    const client = new QueryClient();
    client.setQueryData(keys.dashpayStatus("a"), { kind: "noIdentity", reason: "waitingForSync" });
    client.setQueryData(keys.dashpayStatus("b"), { kind: "ready", main: "x" });
    client.setQueryData(keys.dashpayStatus("c"), { kind: "startupIncomplete", startup: "notRun" });
    client.setQueryData(keys.identities("a"), []);
    let emit: (e: AppEvent) => void = () => undefined;
    const fake = { onEvent: async (l: (e: AppEvent) => void) => ((emit = l), () => undefined) } as unknown as Backend;
    await bridged(fake, client, "testnet");
    emit({ kind: "spvStateChanged", network: "testnet", running: true });
    expect(client.getQueryState(keys.dashpayStatus("a"))?.isInvalidated).toBe(true);
    expect(client.getQueryState(keys.dashpayStatus("b"))?.isInvalidated).toBe(false);
    expect(client.getQueryState(keys.dashpayStatus("c"))?.isInvalidated).toBe(true);
    expect(client.getQueryState(keys.identities("a"))?.isInvalidated).toBe(false);
  });

  it("sync re-reads restored only while it is set: the banner goes once the engine clears RESTORED", async () => {
    const client = new QueryClient();
    client.setQueryData(keys.restored, true);
    let emit: (e: AppEvent) => void = () => undefined;
    const fake = { onEvent: async (l: (e: AppEvent) => void) => ((emit = l), () => undefined), syncStatus: () => new Promise(() => undefined) } as unknown as Backend;
    await bridged(fake, client, "testnet");
    emit({ kind: "sync", network: "testnet", status: {} as never });
    expect(client.getQueryState(keys.restored)?.isInvalidated).toBe(true);
    client.setQueryData(keys.restored, false);
    emit({ kind: "sync", network: "testnet", status: {} as never });
    expect(client.getQueryState(keys.restored)?.isInvalidated).toBe(false);
  });

  it("sync re-reads what RESTORED waits for, with no timer of its own", async () => {
    const client = new QueryClient();
    client.setQueryData(keys.restoreStatus, { waiting: ["platformPass"], historyWallets: [], clearFailed: false });
    let emit: (e: AppEvent) => void = () => undefined;
    const fake = { onEvent: async (l: (e: AppEvent) => void) => ((emit = l), () => undefined), syncStatus: () => new Promise(() => undefined) } as unknown as Backend;
    await bridged(fake, client, "testnet");
    expect(client.getQueryState(keys.restoreStatus)?.isInvalidated).toBe(false);
    emit({ kind: "sync", network: "testnet", status: {} as never });
    expect(client.getQueryState(keys.restoreStatus)?.isInvalidated).toBe(true);
    client.setQueryData(keys.restoreStatus, null);
    emit({ kind: "sync", network: "testnet", status: {} as never });
    expect(client.getQueryState(keys.restoreStatus)?.isInvalidated).toBe(false);
  });

  it("sync reads what RESTORED waits for again after a first read that failed", async () => {
    const client = new QueryClient();
    let reads = 0;
    const observer = new QueryObserver(client, { queryKey: keys.restoreStatus, retry: false, queryFn: () => (++reads === 1 ? Promise.reject(new Error("offline")) : Promise.resolve(null)) });
    const stop = observer.subscribe(() => undefined);
    await vi.waitFor(() => expect(observer.getCurrentResult().isError).toBe(true));
    let emit: (e: AppEvent) => void = () => undefined;
    const fake = { onEvent: async (l: (e: AppEvent) => void) => ((emit = l), () => undefined), syncStatus: () => new Promise(() => undefined) } as unknown as Backend;
    await bridged(fake, client, "testnet");
    expect(reads).toBe(1);
    emit({ kind: "sync", network: "testnet", status: {} as never });
    await vi.waitFor(() => expect(reads).toBe(2));
    stop();
  });

  // Row 74: after a network switch the page is the new network's; what a command still running
  // on the session it left reports is dropped.
  it("knows the events of another network", () => {
    expect(ofOtherNetwork({ kind: "sessionClosed", network: "testnet" }, "mainnet")).toBe(true);
    expect(ofOtherNetwork({ kind: "sessionClosed", network: "mainnet" }, "mainnet")).toBe(false);
    expect(ofOtherNetwork({ kind: "notice", network: "testnet", code: "uncleanShutdown", detail: "" } as AppEvent, "mainnet")).toBe(true);
    expect(ofOtherNetwork({ kind: "notice", network: null, code: "uncleanShutdown", detail: "" } as AppEvent, "mainnet")).toBe(false);
    expect(ofOtherNetwork({ kind: "eventsOverflowed" } as AppEvent, "mainnet")).toBe(false);
    expect(ofOtherNetwork({ kind: "rates" } as AppEvent, "mainnet")).toBe(false);
  });

  it("drops the events of another network: no report, no invalidation, no announcement", async () => {
    const client = new QueryClient();
    const mine = { ...REPORT, flowsOmitted: 2 };
    client.setQueryData(keys.lastLockReport, mine);
    client.setQueryData(keys.balance("w"), null);
    client.setQueryData(keys.wallets, []);
    let emit: (e: AppEvent) => void = () => undefined;
    const calls: string[] = [];
    const fake = {
      onEvent: async (l: (e: AppEvent) => void) => ((emit = l), () => undefined),
      txNotices: async () => (calls.push("txNotices"), []),
    } as unknown as Backend;
    await bridged(fake, client, "mainnet");
    emit({ kind: "lockProgress", network: "testnet", progress: { kind: "done", report: REPORT } });
    emit({ kind: "lockProgress", network: "testnet", progress: { kind: "draining", inFlight: 1, deadlineInMs: 500 } });
    emit({ kind: "balances", network: "testnet", walletId: "w", balances: null });
    emit({ kind: "sessionClosed", network: "testnet" });
    emit({ kind: "newTransactions", network: "testnet", walletId: "w", txids: ["t"], live: [true], count: 1, catchUp: false });
    await new Promise((r) => setTimeout(r, 0));
    expect(client.getQueryData(keys.lastLockReport)).toEqual(mine);
    expect(client.getQueryData(keys.lockDrain)).toBeUndefined();
    expect(client.getQueryState(keys.balance("w"))?.isInvalidated).toBe(false);
    expect(client.getQueryState(keys.wallets)?.isInvalidated).toBe(false);
    expect(calls).toEqual([]);
    // The window's own network still goes through.
    emit({ kind: "balances", network: "mainnet", walletId: "w", balances: null });
    expect(client.getQueryState(keys.balance("w"))?.isInvalidated).toBe(true);
  });

  it("looks up a payment that arrives while locked only at the unlock, for the bell, without a toast", async () => {
    resetReceived();
    const client = new QueryClient();
    const lock = (state: LockStatus["state"]) => seedLockStatus(client, { ...LOCK, state }, lockGeneration());
    lock("locked");
    let emit: (e: AppEvent) => void = () => undefined;
    const asked: string[][] = [];
    const fake = {
      onEvent: async (l: (e: AppEvent) => void) => ((emit = l), () => undefined),
      txNotices: async (_w: string, txids: string[]) => (asked.push(txids), txids.map((txid) => ({ ...NOTICE, txid }))),
    } as unknown as Backend;
    const toasts: unknown[] = [];
    const stopToasts = onReceived((b) => toasts.push(b));
    await bridged(fake, client, "testnet");
    emit({ kind: "newTransactions", network: "testnet", walletId: "w", txids: ["a"], catchUp: false } as AppEvent);
    emit({ kind: "newTransactions", network: "testnet", walletId: "w", txids: ["a", "b"], catchUp: false } as AppEvent);
    await new Promise((r) => setTimeout(r, 0));
    expect(asked).toEqual([]);
    lock("unlockedMixingOnly");
    await new Promise((r) => setTimeout(r, 0));
    expect(asked).toEqual([["b", "a"]]);
    lock("unlocked");
    await new Promise((r) => setTimeout(r, 0));
    expect(asked).toHaveLength(1);
    expect(toasts).toEqual([]);
    emit({ kind: "newTransactions", network: "testnet", walletId: "w", txids: ["c"], catchUp: false } as AppEvent);
    await new Promise((r) => setTimeout(r, 0));
    expect(asked).toEqual([["b", "a"], ["c"]]);
    expect(toasts).toHaveLength(1);
    stopToasts();
    resetReceived();
  });

  it("closes the gate on the lock event itself, before the status is read again, and drops the wallet's queries", async () => {
    resetReceived();
    const client = new QueryClient();
    seedLockStatus(client, { ...LOCK, state: "unlocked" }, lockGeneration());
    client.setQueryData(keys.balance("w"), "32.04698287");
    let emit: (e: AppEvent) => void = () => undefined;
    const asked: string[][] = [];
    const statusReads: unknown[] = [];
    const fake = {
      onEvent: async (l: (e: AppEvent) => void) => ((emit = l), () => undefined),
      lockStatus: () => new Promise((r) => statusReads.push(r)),
      txNotices: async (_w: string, txids: string[]) => (asked.push(txids), txids.map((txid) => ({ ...NOTICE, txid }))),
    } as unknown as Backend;
    await bridged(fake, client, "testnet");
    expect(walletGateClosed()).toBe(false);
    emit({ kind: "vaultLockState", network: "testnet", state: "locked" } as AppEvent);
    expect(walletGateClosed()).toBe(true);
    expect(client.getQueryData<LockStatus>(keys.lockStatus)?.state).toBe("locked");
    expect(client.getQueryCache().find({ queryKey: keys.balance("w") })).toBeUndefined();
    // A payment in the same interval waits for the unlock.
    emit({ kind: "newTransactions", network: "testnet", walletId: "w", txids: ["a"], catchUp: false } as AppEvent);
    await new Promise((r) => setTimeout(r, 0));
    expect(asked).toEqual([]);
    seedLockStatus(client, { ...LOCK, state: "unlocked" }, lockGeneration());
    await new Promise((r) => setTimeout(r, 0));
    expect(walletGateClosed()).toBe(false);
    expect(asked).toEqual([["a"]]);
    resetReceived();
  });

  it("ends a confirmation with the bridge", async () => {
    const client = new QueryClient();
    seedLockStatus(client, { ...LOCK, state: "unlocked" }, lockGeneration());
    let emit: (e: AppEvent) => void = () => undefined;
    let statusReads = 0;
    const fake = {
      onEvent: async (l: (e: AppEvent) => void) => ((emit = l), () => undefined),
      lockStatus: () => (statusReads++, new Promise(() => undefined)),
    } as unknown as Backend;
    const stop = await bridged(fake, client, "testnet");
    vi.useFakeTimers();
    try {
      emit({ kind: "eventsOverflowed", network: "testnet" } as AppEvent);
      expect(statusReads).toBe(1);
      expect(lockUnconfirmed()).toBe(true);
      stop();
      expect(lockUnconfirmed()).toBe(false);
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it("drops the payments held for a removed wallet and its governance reads, and all the payments on a network switch", async () => {
    resetReceived();
    const client = new QueryClient();
    seedLockStatus(client, LOCK, lockGeneration());
    let emit: (e: AppEvent) => void = () => undefined;
    const asked: [string, string[]][] = [];
    const fake = {
      onEvent: async (l: (e: AppEvent) => void) => ((emit = l), () => undefined),
      txNotices: async (w: string, txids: string[]) => (asked.push([w, txids]), txids.map((txid) => ({ ...NOTICE, txid }))),
    } as unknown as Backend;
    await bridged(fake, client, "testnet");
    const paid = (walletId: string, txid: string) => emit({ kind: "newTransactions", network: "testnet", walletId, txids: [txid], catchUp: false } as AppEvent);
    paid("w", "a");
    paid("v", "b");
    client.setQueryData([...keys.governanceAll, "mine", "v"], [{ name: "p" }]);
    client.setQueryData([...keys.governanceAll, "mine", "w"], [{ name: "q" }]);
    emit({ kind: "walletRemoved", network: "testnet", walletId: "v" } as AppEvent);
    expect(client.getQueryData([...keys.governanceAll, "mine", "v"])).toBeUndefined();
    expect(client.getQueryData([...keys.governanceAll, "mine", "w"])).toEqual([{ name: "q" }]);
    seedLockStatus(client, { ...LOCK, state: "unlocked" }, lockGeneration());
    await new Promise((r) => setTimeout(r, 0));
    expect(asked).toEqual([["w", ["a"]]]);
    seedLockStatus(client, LOCK, lockGeneration());
    paid("w", "c");
    emit({ kind: "sessionClosed", network: "testnet" } as AppEvent);
    seedLockStatus(client, { ...LOCK, state: "unlocked" }, lockGeneration());
    await new Promise((r) => setTimeout(r, 0));
    expect(asked).toHaveLength(1);
    resetReceived();
  });

  it("drops the bell's payments on a lock and reads them again after the unlock, a lookup that answers locked recording nothing", async () => {
    resetReceived();
    const client = new QueryClient();
    seedLockStatus(client, { ...LOCK, state: "unlocked" }, lockGeneration());
    let emit: (e: AppEvent) => void = () => undefined;
    const asked: string[][] = [];
    let slow: ((rows: TxNoticeRow[]) => void) | null = null;
    const fake = {
      onEvent: async (l: (e: AppEvent) => void) => ((emit = l), () => undefined),
      txNotices: (_w: string, txids: string[]) => {
        asked.push(txids);
        const rows = txids.map((txid) => ({ ...NOTICE, txid, address: "yPaid" }));
        return slow === null ? Promise.resolve(rows) : new Promise((r) => (slow = () => r(rows)));
      },
    } as unknown as Backend;
    const stop = await bridged(fake, client, "testnet");
    emit({ kind: "newTransactions", network: "testnet", walletId: "w", txids: ["a"], catchUp: false } as AppEvent);
    await new Promise((r) => setTimeout(r, 0));
    expect(receivedNow().map((p) => p.txid)).toEqual(["a"]);
    const lock = (state: LockStatus["state"]) => seedLockStatus(client, { ...LOCK, state }, lockGeneration());
    lock("locked");
    expect(receivedNow()).toEqual([]);
    // The unlock's lookup is slow, and the vault locks again before it answers.
    slow = () => undefined;
    lock("unlocked");
    await new Promise((r) => setTimeout(r, 0));
    expect(asked).toEqual([["a"], ["a"]]);
    lock("locked");
    (slow as unknown as () => void)();
    await new Promise((r) => setTimeout(r, 0));
    expect(receivedNow()).toEqual([]);
    slow = null;
    lock("unlocked");
    await new Promise((r) => setTimeout(r, 0));
    expect(receivedNow().map((p) => [p.txid, p.address])).toEqual([["a", "yPaid"]]);
    stop();
    resetReceived();
  });
});

// Issue #141: a payment learned live is announced while the wallet catches up; the scan's findings are not.
describe("announcing new transactions", () => {
  type NewTxs = Extract<AppEvent, { kind: "newTransactions" }>;
  const batch = (txids: string[], live: boolean[], catchUp: boolean): NewTxs => ({ kind: "newTransactions", network: "testnet", walletId: "w", txids, live, count: txids.length, catchUp });

  it("announces the live ones of a catch-up batch, and every one once caught up or near the tip", () => {
    const mixed = batch(["scanned", "mempool", "locked"], [false, true, true], true);
    expect(toAnnounce(mixed, false)).toEqual(["mempool", "locked"]);
    expect(toAnnounce(mixed, true)).toEqual(["scanned", "mempool", "locked"]);
    expect(toAnnounce({ ...mixed, catchUp: false }, false)).toEqual(["scanned", "mempool", "locked"]);
    expect(toAnnounce(batch(["scanned"], [false], true), false)).toEqual([]);
  });

  it("looks up only the live payments while catching up, and none of a restore's scanned history", async () => {
    const client = new QueryClient();
    let emit: (e: AppEvent) => void = () => undefined;
    const asked: string[][] = [];
    const fake = {
      onEvent: async (l: (e: AppEvent) => void) => ((emit = l), () => undefined),
      txNotices: async (_: string, txids: string[]) => (asked.push(txids), []),
    } as unknown as Backend;
    await bridged(fake, client, "testnet");
    const history = Array.from({ length: 256 }, (_, i) => `old${i}`);
    emit(batch(history, history.map(() => false), true));
    emit(batch(["old", "paid"], [false, true], true));
    await new Promise((r) => setTimeout(r, 0));
    expect(asked).toEqual([["paid"]]);
  });

  it("holds only the live payments of a catch-up batch that arrives while locked, and looks them up at the unlock", async () => {
    resetReceived();
    const client = new QueryClient();
    seedLockStatus(client, LOCK, lockGeneration());
    let emit: (e: AppEvent) => void = () => undefined;
    const asked: string[][] = [];
    const fake = {
      onEvent: async (l: (e: AppEvent) => void) => ((emit = l), () => undefined),
      txNotices: async (_: string, txids: string[]) => (asked.push(txids), txids.map((txid) => ({ ...NOTICE, txid }))),
    } as unknown as Backend;
    await bridged(fake, client, "testnet");
    expect(walletGateClosed()).toBe(true);
    emit(batch(["scanned", "paid"], [false, true], true));
    await new Promise((r) => setTimeout(r, 0));
    expect(asked).toEqual([]);
    seedLockStatus(client, { ...LOCK, state: "unlocked" }, lockGeneration());
    await new Promise((r) => setTimeout(r, 0));
    expect(asked).toEqual([["paid"]]);
    expect(receivedNow().map((p) => p.txid)).toEqual(["paid"]);
    resetReceived();
  });
});
