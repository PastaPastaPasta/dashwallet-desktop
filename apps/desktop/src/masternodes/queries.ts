// My masternodes' reads (MG-10 §3.1, §3.5). Engine-local reads sit under `["masternodes", …]`, so a
// `masternodes` event re-reads them all; an evonode's credits come from Platform and sit under
// `["mnPlatform", hash, "credits"]`, re-read on a timer instead. `mn_reveal_key` has no query: its
// result is a secret the sheet holds only while shown.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { MasternodeKeyRoleName } from "../api/generated/MasternodeKeyRoleName";
import { keys, useBackend } from "../api/queries";

const m = keys.masternodesAll;
/** Platform reads while shown (§3.5). */
export const PLATFORM_REFRESH_MS = 60_000;

export const mnKeys = {
  listState: [...m, "listState"] as const,
  listAll: [...m, "list"] as const,
  list: (walletId: string) => [...m, "list", walletId] as const,
  detailAll: [...m, "detail"] as const,
  detail: (walletId: string, hash: string) => [...m, "detail", walletId, hash] as const,
  payoutsAll: (walletId: string) => [...m, "payouts", walletId] as const,
  payouts: (walletId: string, hash: string, since: number | null) => [...m, "payouts", walletId, hash, since] as const,
  find: (query: string) => [...m, "find", query] as const,
  keychain: (walletId: string, role: MasternodeKeyRoleName, start: number, count: number) => [...m, "keychain", walletId, role, start, count] as const,
  credits: (hash: string) => ["mnPlatform", hash, "credits"] as const,
};

export function useMasternodeListState() {
  const b = useBackend();
  return useQuery({ queryKey: mnKeys.listState, queryFn: () => b.mnListState() });
}

export function useMasternodes(walletId: string, enabled = true) {
  const b = useBackend();
  return useQuery({ queryKey: mnKeys.list(walletId), queryFn: () => b.mnList(walletId), enabled });
}

export function useMasternode(walletId: string, hash: string, enabled = true) {
  const b = useBackend();
  return useQuery({ queryKey: mnKeys.detail(walletId, hash), queryFn: () => b.mnMasternode(walletId, hash), enabled });
}

export function useMasternodePayouts(walletId: string, hash: string, since: number | null, enabled = true) {
  const b = useBackend();
  return useQuery({ queryKey: mnKeys.payouts(walletId, hash, since), queryFn: () => b.mnPayouts(walletId, hash, since), enabled });
}

/** Track's search; the results stay while the query changes. */
export function useFindMasternodes(query: string, enabled: boolean) {
  const b = useBackend();
  return useQuery({ queryKey: mnKeys.find(query), queryFn: () => b.mnFind(query), enabled, placeholderData: (prev) => prev });
}

export function useEvonodeCredits(hash: string, enabled = true) {
  const b = useBackend();
  return useQuery({ queryKey: mnKeys.credits(hash), queryFn: () => b.mnEvonodeCredits(hash), enabled, refetchInterval: PLATFORM_REFRESH_MS });
}

/** The keychain's public keys of `role`, one page. */
export function useMasternodeKeys(walletId: string, role: MasternodeKeyRoleName, start: number, count: number) {
  const b = useBackend();
  return useQuery({ queryKey: mnKeys.keychain(walletId, role, start, count), queryFn: () => b.mnKeys(walletId, role, start, count) });
}

/** Re-reads every engine-local masternode query (after a command, before its event lands). */
export function useMasternodesRefresh() {
  const client = useQueryClient();
  return () => client.invalidateQueries({ queryKey: m });
}
