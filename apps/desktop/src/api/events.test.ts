import { QueryClient } from "@tanstack/react-query";
import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { InfiniteData } from "@tanstack/react-query";
import { bridgeEvents, invalidationsFor, RECONCILE_COALESCE_MS, RECONCILE_MAX_WAIT_MS } from "./events";
import type { Backend } from "./backend";
import type { AppEvent } from "./generated/commands";
import type { HistoryRows } from "./generated/HistoryRows";
import type { LockOutcome } from "./generated/LockOutcome";
import { installFixture } from "../fixture";
import { FIXTURE_WALLET_ID } from "../fixture/scenarios";
import { profileKeys } from "../dashpay/profileQueries";
import { dashpayKeys } from "../dashpay/queries";
import { keys } from "./queries";
import { IpcBackend } from "./tauri";

const REPORT: LockOutcome = {
  status: { state: "locked", encrypted: true, quickUnlockEnrolled: false, failedAttempts: 0, retryAfterSecs: null, walletsWithSecrets: [] },
  flows: [],
  flowsOmitted: 0,
  walletsWithSecretsOmitted: 0,
};

beforeEach(() => vi.stubGlobal("window", globalThis));
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

describe("event bridge", () => {
  it("maps each signal to the queries it touches", () => {
    expect(invalidationsFor({ kind: "historyChanged", network: "testnet", walletId: "w", txids: [] })).toEqual([
      ["history", "w"],
      ["txDetail", "w"],
      ["receive", "w"],
      ["coins", "w"],
      ["dashpay", "contacts", "w"],
    ]);
    expect(invalidationsFor({ kind: "balances", network: "testnet", walletId: "w", balances: null })).toEqual([
      keys.balance("w"),
      keys.wallets,
      keys.coinsAll("w"),
      dashpayKeys.quote("w"),
      keys.coinjoinStatus("w"),
    ]);
    expect(invalidationsFor({ kind: "vaultLockState", network: "testnet", state: "locked" })).toEqual([keys.lockStatus, keys.wallets, keys.lockReconcile, dashpayKeys.registrationsAll]);
    expect(invalidationsFor({ kind: "leaseChanged", network: "testnet", lease: {} as never })).toEqual([keys.lockReconcile]);
    // DP1-09 D14: a registration lease's change re-reads the rows, whose `waiting` moved.
    const registration = { flow: "registration" } as never;
    expect(invalidationsFor({ kind: "leaseChanged", network: "testnet", lease: registration })).toEqual([keys.lockReconcile, dashpayKeys.registrationsAll]);
    // A settled funding step re-reads the stuck top-up list (DP1-10 D3).
    expect(invalidationsFor({ kind: "dispatchResolved", network: "testnet", settled: {} as never })).toEqual([keys.lockReconcile, profileKeys.stuckTopUpsAll]);
    expect(invalidationsFor({ kind: "sessionOpened", network: "testnet" })).toBe("all");
    expect(invalidationsFor({ kind: "eventsOverflowed" })).toBe("all");
    expect(invalidationsFor({ kind: "lockProgress", network: "testnet", progress: { kind: "draining", inFlight: 1, deadlineInMs: 500 } })).toEqual([]);
    expect(invalidationsFor({ kind: "platform", network: "testnet", walletId: "w", change: { domain: "names" } })).toEqual([keys.dashpayAll, keys.receiveAll("w")]);
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
    expect(invalidationsFor({ kind: "spvStateChanged", network: "testnet", running: true })).toEqual([keys.sync, keys.dashpayStatusAll]);
    expect(invalidationsFor({ kind: "sync", network: "testnet", status: {} as never })).toEqual([keys.sync, keys.dashpayStatusAll]);
    // U-06: the engine's CoinJoin signal, and a balance change (D7), re-read the wallet's mixing status.
    expect(invalidationsFor({ kind: "coinJoin", network: "testnet", walletId: "w" })).toEqual([keys.coinjoinStatus("w")]);
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
      await bridgeEvents(fake, client);
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
    await bridgeEvents(backend, client);
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
      const unsubscribe = await bridgeEvents(fake, client);
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

  it("invalidates the lock status when the fixture vault locks", async () => {
    installFixture({ scenario: "funded" });
    const backend = new IpcBackend("fixture");
    const client = new QueryClient();
    client.setQueryData(keys.lockStatus, await backend.lockStatus());
    client.setQueryData(keys.balance(FIXTURE_WALLET_ID), await backend.balance(FIXTURE_WALLET_ID));
    await bridgeEvents(backend, client);
    await backend.lock();
    expect(client.getQueryState(keys.lockStatus)?.isInvalidated).toBe(true);
    expect(client.getQueryState(keys.balance(FIXTURE_WALLET_ID))?.isInvalidated).toBe(false);
  });

  it("cuts a loaded history list back to its first page before refetching", async () => {
    const client = new QueryClient();
    const key = keys.history(FIXTURE_WALLET_ID, { categories: [], text: null, sort: "newestFirst" }, 2);
    const page = { rows: [], nextCursor: "x", totalMatching: 4 };
    client.setQueryData<InfiniteData<HistoryRows, string | null>>(key, { pages: [page, page], pageParams: [null, "2"] });
    let emit: (e: AppEvent) => void = () => undefined;
    const fake = { onEvent: async (l: (e: AppEvent) => void) => ((emit = l), () => undefined) } as unknown as Backend;
    await bridgeEvents(fake, client);
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
    await bridgeEvents(fake, client);
    emit({ kind: "walletCreated", network: "testnet", walletId: "a" });
    expect(client.getQueryState(keys.dashpayStatus("a"))?.isInvalidated).toBe(true);
    expect(client.getQueryState(keys.dashpayStatus("b"))?.isInvalidated).toBe(false);
  });

  it("sync re-reads only the DashPay statuses without an identity (BL-153 D9, DP1-09 D28)", async () => {
    const client = new QueryClient();
    client.setQueryData(keys.dashpayStatus("a"), { kind: "noIdentity", reason: "waitingForSync" });
    client.setQueryData(keys.dashpayStatus("b"), { kind: "ready", main: "x" });
    client.setQueryData(keys.dashpayStatus("c"), { kind: "startupIncomplete", startup: "notRun" });
    client.setQueryData(keys.identities("a"), []);
    let emit: (e: AppEvent) => void = () => undefined;
    const fake = { onEvent: async (l: (e: AppEvent) => void) => ((emit = l), () => undefined) } as unknown as Backend;
    await bridgeEvents(fake, client);
    emit({ kind: "spvStateChanged", network: "testnet", running: true });
    expect(client.getQueryState(keys.dashpayStatus("a"))?.isInvalidated).toBe(true);
    expect(client.getQueryState(keys.dashpayStatus("b"))?.isInvalidated).toBe(false);
    expect(client.getQueryState(keys.dashpayStatus("c"))?.isInvalidated).toBe(true);
    expect(client.getQueryState(keys.identities("a"))?.isInvalidated).toBe(false);
  });
});
