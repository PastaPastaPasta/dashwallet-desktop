// Server state lives in TanStack Query. Keys mirror the dw-app commands, so an engine
// signal names exactly the queries to refetch (events.ts).

import { createContext, useContext } from "react";
import { useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import type { Backend } from "./backend";
import type { HistoryRequest } from "./generated/HistoryRequest";
import type { LockOutcome } from "./generated/LockOutcome";
import { reconcileLockReport } from "./lockReport";

/** A history query without its paging fields. */
export type HistoryFilter = Omit<HistoryRequest, "cursor" | "limit">;

export const BackendContext = createContext<Backend | null>(null);

export function useBackend(): Backend {
  const b = useContext(BackendContext);
  if (!b) throw new Error("BackendContext missing");
  return b;
}

export const keys = {
  network: ["network"] as const,
  lockStatus: ["lockStatus"] as const,
  lastLockReport: ["lastLockReport"] as const,
  /** Reads `leases`, `dispatchStatus` and `lockStatus` (lockReport.ts). */
  lockReconcile: ["lockReconcile"] as const,
  wallets: ["wallets"] as const,
  balance: (walletId: string) => ["balance", walletId] as const,
  sync: ["sync"] as const,
  historyAll: (walletId: string) => ["history", walletId] as const,
  history: (walletId: string, filter: HistoryFilter, limit: number) => ["history", walletId, filter, limit] as const,
  txDetailAll: (walletId: string) => ["txDetail", walletId] as const,
  txDetail: (walletId: string, txid: string) => ["txDetail", walletId, txid] as const,
};

/** Opens the network; its lock status seeds `lockStatus`, so the shell never renders
 * one frame unlocked before the lock state is known. Resolves to the lock status. */
export function useOpenNetwork() {
  const b = useBackend();
  const client = useQueryClient();
  return useQuery({
    queryKey: keys.network,
    queryFn: async () => {
      const { lock } = await b.openNetwork();
      client.setQueryData(keys.lockStatus, lock);
      return lock;
    },
  });
}

export function useLockStatus() {
  const b = useBackend();
  return useQuery({ queryKey: keys.lockStatus, queryFn: () => b.lockStatus() });
}

export function useWallets() {
  const b = useBackend();
  return useQuery({ queryKey: keys.wallets, queryFn: () => b.listWallets() });
}

export function useBalance(walletId: string) {
  const b = useBackend();
  return useQuery({ queryKey: keys.balance(walletId), queryFn: () => b.balance(walletId) });
}

export function useSyncStatus() {
  const b = useBackend();
  return useQuery({ queryKey: keys.sync, queryFn: () => b.syncStatus() });
}

/** Pages of `limit` records; `fetchNextPage` follows the engine's keyset cursor. */
export function useHistory(walletId: string, filter: HistoryFilter, limit: number) {
  const b = useBackend();
  return useInfiniteQuery({
    queryKey: keys.history(walletId, filter, limit),
    queryFn: ({ pageParam }) => b.historyPage(walletId, { ...filter, cursor: pageParam, limit }),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.nextCursor,
  });
}

export function useTxDetail(walletId: string, txid: string) {
  const b = useBackend();
  return useQuery({ queryKey: keys.txDetail(walletId, txid), queryFn: () => b.txDetail(walletId, txid) });
}

/** The network's last lock report (E0-13 D3); `null` until something locks the vault. */
export function useLastLockReport() {
  const b = useBackend();
  return useQuery({ queryKey: keys.lastLockReport, queryFn: () => b.lastLockReport() });
}

/** What the engine knows now about what `report` left pending or left out. */
export function useLockReconcile(report: LockOutcome | null | undefined, enabled: boolean) {
  const b = useBackend();
  return useQuery({
    queryKey: [...keys.lockReconcile, report] as const,
    queryFn: () => reconcileLockReport(report!, b),
    enabled: enabled && !!report,
  });
}
