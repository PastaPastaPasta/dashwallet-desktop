import { QueryClient } from "@tanstack/react-query";
import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { InfiniteData } from "@tanstack/react-query";
import { bridgeEvents, invalidationsFor, RECONCILE_COALESCE_MS } from "./events";
import type { Backend } from "./backend";
import type { AppEvent } from "./generated/commands";
import type { HistoryRows } from "./generated/HistoryRows";
import type { LockOutcome } from "./generated/LockOutcome";
import { installFixture } from "../fixture";
import { FIXTURE_WALLET_ID } from "../fixture/scenarios";
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
    expect(invalidationsFor({ kind: "historyChanged", network: "testnet", walletId: "w", txids: [] })).toEqual([["history", "w"], ["txDetail", "w"]]);
    expect(invalidationsFor({ kind: "vaultLockState", network: "testnet", state: "locked" })).toEqual([keys.lockStatus, keys.wallets, keys.lockReconcile]);
    expect(invalidationsFor({ kind: "leaseChanged", network: "testnet", lease: {} as never })).toEqual([keys.lockReconcile]);
    expect(invalidationsFor({ kind: "sessionOpened", network: "testnet" })).toBe("all");
    expect(invalidationsFor({ kind: "eventsOverflowed" })).toBe("all");
    expect(invalidationsFor({ kind: "lockProgress", network: "testnet", progress: { kind: "draining", inFlight: 1, deadlineInMs: 500 } })).toEqual([]);
    expect(invalidationsFor({ kind: "platform", network: "testnet", walletId: "w", change: { domain: "names" } })).toEqual([]);
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
});
