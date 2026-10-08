// The renderer half of the event bridge: each engine signal invalidates the queries that
// read what changed. Rows are never pushed; the queries refetch (DESIGN-opus §1.5 rule 4).

import type { QueryClient } from "@tanstack/react-query";
import type { Backend } from "./backend";
import type { EngineSignal } from "./generated/EngineSignal";
import { keys } from "./queries";

export function invalidationsFor(signal: EngineSignal): (readonly unknown[])[] {
  switch (signal.kind) {
    case "session":
      return [[]]; // every query
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
    for (const queryKey of invalidationsFor(signal)) void client.invalidateQueries({ queryKey });
  });
}
