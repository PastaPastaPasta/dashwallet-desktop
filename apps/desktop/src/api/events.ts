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
      return [keys.lockStatus, keys.wallets];
    case "lockProgress":
      return event.progress.kind === "done" ? [keys.lockStatus] : [];
    // No screen reads these yet (U-07 adds the DashPay, CoinJoin and lease views).
    case "newTransactions":
    case "dashPayNews":
    case "coinJoin":
    case "platform":
    case "leaseChanged":
    case "dispatchResolved":
    case "notice":
      return [];
    default:
      return unreachable(event);
  }
}

function unreachable(event: never): never {
  throw new Error(`unhandled engine event ${JSON.stringify(event)}`);
}

/** Subscribes the query client to the backend's events; returns the unsubscribe function. */
export async function bridgeEvents(backend: Backend, client: QueryClient): Promise<() => void> {
  return backend.onEvent((event) => {
    if (event.kind === "notice") console.warn("engine notice", event.code, event.detail);
    if (event.kind === "historyChanged") {
      // Refetch only the first page of each list: every history_page call re-reads the whole
      // history in the engine, and during sync events arrive up to 4 times a second.
      client.setQueriesData<InfiniteData<HistoryRows, string | null>>({ queryKey: keys.historyAll(event.walletId) }, (d) =>
        d && d.pages.length > 1 ? { pages: d.pages.slice(0, 1), pageParams: d.pageParams.slice(0, 1) } : d,
      );
    }
    const stale = invalidationsFor(event);
    if (stale === "all") void client.invalidateQueries({ predicate: (q) => q.queryKey[0] !== keys.network[0] });
    else for (const queryKey of stale) void client.invalidateQueries({ queryKey });
  });
}
