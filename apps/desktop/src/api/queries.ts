// Server state lives in TanStack Query. Keys mirror the dw-app commands, so an engine
// signal names exactly the queries to refetch (events.ts).

import { createContext, useContext } from "react";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import type { Backend } from "./backend";
import type { HistoryRequest } from "./generated/HistoryRequest";

export const BackendContext = createContext<Backend | null>(null);

export function useBackend(): Backend {
  const b = useContext(BackendContext);
  if (!b) throw new Error("BackendContext missing");
  return b;
}

export const keys = {
  network: ["network"] as const,
  lockStatus: ["lockStatus"] as const,
  wallets: ["wallets"] as const,
  balance: (walletId: string) => ["balance", walletId] as const,
  sync: ["sync"] as const,
  historyAll: (walletId: string) => ["history", walletId] as const,
  history: (walletId: string, filter: Omit<HistoryRequest, "cursor" | "limit">, limit: number) =>
    ["history", walletId, filter, limit] as const,
  txDetailAll: (walletId: string) => ["txDetail", walletId] as const,
  txDetail: (walletId: string, txid: string) => ["txDetail", walletId, txid] as const,
};

export const useLockStatus = () => {
  const b = useBackend();
  return useQuery({ queryKey: keys.lockStatus, queryFn: () => b.lockStatus() });
};

export const useWallets = () => {
  const b = useBackend();
  return useQuery({ queryKey: keys.wallets, queryFn: () => b.listWallets() });
};

export const useBalance = (walletId: string | undefined) => {
  const b = useBackend();
  return useQuery({ queryKey: keys.balance(walletId ?? ""), queryFn: () => b.balance(walletId!), enabled: !!walletId });
};

export const useSyncStatus = () => {
  const b = useBackend();
  return useQuery({ queryKey: keys.sync, queryFn: () => b.syncStatus() });
};

/** Pages of `limit` records; `fetchNextPage` follows the engine's cursor. */
export function useHistory(walletId: string | undefined, filter: Omit<HistoryRequest, "cursor" | "limit">, limit: number) {
  const b = useBackend();
  return useInfiniteQuery({
    queryKey: keys.history(walletId ?? "", filter, limit),
    queryFn: ({ pageParam }) => b.historyPage(walletId!, { ...filter, cursor: pageParam, limit }),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.nextCursor,
    enabled: !!walletId,
  });
}

export const useTxDetail = (walletId: string | undefined, txid: string | null) => {
  const b = useBackend();
  return useQuery({
    queryKey: keys.txDetail(walletId ?? "", txid ?? ""),
    queryFn: () => b.txDetail(walletId!, txid!),
    enabled: !!walletId && !!txid,
  });
};

