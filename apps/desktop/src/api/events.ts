// The renderer half of the event bridge: each engine event invalidates the queries that
// read what changed. Rows are never pushed; the queries refetch (DESIGN-opus §1.5 rule 4).
// Events arrive in the engine's order on one channel (`ENGINE_EVENT`); see dw-app's bridge.rs.
// A queue overflow drops events and sends one `eventsOverflowed`, which reloads everything.

import type { InfiniteData, QueryClient, QueryKey } from "@tanstack/react-query";
import type { Backend } from "./backend";
import type { AppEvent } from "./generated/commands";
import type { HistoryRows } from "./generated/HistoryRows";
import { keys } from "./queries";

/** The query keys an event makes stale; `"all"` means every query but the network itself. */
export function invalidationsFor(event: AppEvent): QueryKey[] | "all" {
  switch (event.kind) {
    case "sessionOpened":
    case "sessionClosed":
    // The app's queue overflowed and dropped events: whatever they said, the queries know.
    case "eventsOverflowed":
      return "all";
    case "walletCreated":
    case "walletRemoved":
    case "walletLoadChanged":
      return [keys.wallets];
    case "spvStateChanged":
    case "sync":
      return [keys.sync];
    case "balances":
      return [keys.balance(event.walletId), keys.wallets];
    case "historyChanged":
      return [keys.historyAll(event.walletId), keys.txDetailAll(event.walletId)];
    case "vaultLockState":
      return [keys.lockStatus, keys.wallets, keys.lockReconcile];
    case "lockProgress":
      // `done` carries the report itself (bridgeEvents sets it).
      return event.progress.kind === "done" ? [keys.lockStatus, keys.lockReconcile] : [];
    // The lock report's reconciliation reads leases and dispatch states (lockReport.ts); its
    // invalidations are coalesced (RECONCILE_COALESCE_MS).
    case "leaseChanged":
    case "dispatchResolved":
      return [keys.lockReconcile];
    // No screen reads these yet (the DashPay, CoinJoin and notice views come with their tasks).
    case "newTransactions":
    case "coinJoin":
    case "platform":
    case "notice":
      return [];
    default:
      return unreachable(event);
  }
}

function unreachable(event: never): never {
  throw new Error(`unhandled engine event ${JSON.stringify(event)}`);
}

/** A lock that cuts 16 flows sends a `leaseChanged` for each, and every reconciliation reads
 * `leases`, `lockStatus` and up to 128 `dispatchStatus`: one runs this long after the last of a
 * burst, and at most RECONCILE_MAX_WAIT_MS after its first event. */
export const RECONCILE_COALESCE_MS = 100;
export const RECONCILE_MAX_WAIT_MS = 500;

/** Subscribes the query client to the backend's events; returns the unsubscribe function. */
export async function bridgeEvents(backend: Backend, client: QueryClient): Promise<() => void> {
  let reconcile: ReturnType<typeof setTimeout> | undefined;
  let reconcileBy = 0;
  const stopReconcile = () => {
    clearTimeout(reconcile);
    reconcile = undefined;
  };
  const unsubscribe = await backend.onEvent((event) => {
    if (event.kind === "notice") console.warn("engine notice", event.code, event.detail);
    if (event.kind === "historyChanged") {
      // Refetch only the first page of each list: every history_page call re-reads the whole
      // history in the engine, and during sync events arrive up to 4 times a second.
      client.setQueriesData<InfiniteData<HistoryRows, string | null>>({ queryKey: keys.historyAll(event.walletId) }, (d) =>
        d && d.pages.length > 1 ? { pages: d.pages.slice(0, 1), pageParams: d.pageParams.slice(0, 1) } : d,
      );
    }
    if (event.kind === "lockProgress") {
      const { progress } = event;
      // The new report shows with its `done`, never the previous lock's while a refetch runs: a
      // fetch already under way may hold the previous one, so it is dropped first.
      if (progress.kind === "done") {
        void client.cancelQueries({ queryKey: keys.lastLockReport });
        client.setQueryData(keys.lastLockReport, progress.report);
      }
      // While a lock drains, the last report is the previous lock's (useLockDrain).
      client.setQueryData(keys.lockDrain, progress.kind === "draining" ? progress : null);
    }
    const stale = invalidationsFor(event);
    if (stale === "all") {
      stopReconcile();
      void client.invalidateQueries({ predicate: (q) => q.queryKey[0] !== keys.network[0] });
    } else
      for (const queryKey of stale) {
        if (queryKey !== keys.lockReconcile) void client.invalidateQueries({ queryKey });
        else {
          const now = Date.now();
          if (reconcile === undefined) reconcileBy = now + RECONCILE_MAX_WAIT_MS;
          stopReconcile();
          reconcile = setTimeout(
            () => {
              reconcile = undefined;
              void client.invalidateQueries({ queryKey });
            },
            Math.min(RECONCILE_COALESCE_MS, reconcileBy - now),
          );
        }
      }
  });
  return () => {
    stopReconcile();
    unsubscribe();
  };
}
