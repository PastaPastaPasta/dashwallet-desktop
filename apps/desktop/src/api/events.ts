// The renderer half of the event bridge: each engine signal invalidates the queries that
// read what changed. Rows are never pushed; the queries refetch (DESIGN-opus §1.5 rule 4).

import type { InfiniteData, QueryClient, QueryKey } from "@tanstack/react-query";
import type { Backend } from "./backend";
import type { EngineSignal } from "./generated/EngineSignal";
import type { HistoryRows } from "./generated/HistoryRows";
import { keys } from "./queries";

/** The query keys a signal makes stale; `"all"` means every query but the network itself. */
export function invalidationsFor(signal: EngineSignal): QueryKey[] | "all" {
  switch (signal.kind) {
    case "session":
      return "all";
    case "wallets":
      return [keys.wallets];
    case "sync":
      return [keys.sync];
    case "balances":
      return [keys.balance(signal.walletId), keys.wallets];
    case "history":
      return [keys.historyAll(signal.walletId), keys.txDetailAll(signal.walletId)];
    case "lockState":
      return [keys.lockStatus, keys.wallets];
    case "notice":
      return [];
  }
}

/** Subscribes the query client to the backend's signals; returns the unsubscribe function. */
export async function bridgeSignals(backend: Backend, client: QueryClient): Promise<() => void> {
  return backend.onSignal((signal) => {
    if (signal.kind === "notice") console.warn("engine notice", signal.code, signal.detail);
    if (signal.kind === "history") {
      // Refetch only the first page of each list: every history_page call re-reads the whole
      // history in the engine, and during sync signals arrive up to 4 times a second.
      client.setQueriesData<InfiniteData<HistoryRows, string | null>>({ queryKey: keys.historyAll(signal.walletId) }, (d) =>
        d && d.pages.length > 1 ? { pages: d.pages.slice(0, 1), pageParams: d.pageParams.slice(0, 1) } : d,
      );
    }
    const stale = invalidationsFor(signal);
    if (stale === "all") void client.invalidateQueries({ predicate: (q) => q.queryKey[0] !== keys.network[0] });
    else for (const queryKey of stale) void client.invalidateQueries({ queryKey });
  });
}
