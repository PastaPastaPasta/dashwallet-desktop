// My masternodes' reads (MG-10 §3.1, §3.5). Engine-local reads sit under `["masternodes", …]`, so a
// `masternodes` event re-reads them all; an evonode's credits come from Platform and sit under
// `["mnPlatform", hash, "credits"]`, re-read on a timer instead. `mn_reveal_key` has no query: its
// result is a secret the sheet holds only while shown.

import { useMemo } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { MasternodeKeyRoleName } from "../api/generated/MasternodeKeyRoleName";
import type { MasternodeTypeName } from "../api/generated/MasternodeTypeName";
import type { ProTxOperation } from "../api/generated/ProTxOperation";
import { hasCode } from "../api/backend";
import { keys, useBackend } from "../api/queries";
import { useUi } from "../app/ui";
import type { NodeNames } from "../governance/model";
import { nodeNames } from "./model";

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
  operations: (walletId: string) => [...m, "operations", walletId] as const,
  credits: (hash: string) => ["mnPlatform", hash, "credits"] as const,
  sharedCapability: (walletId: string) => [...m, "sharedCapability", walletId] as const,
  sharedSessions: (walletId: string) => [...m, "sharedSessions", walletId] as const,
  sharedUnreadable: (walletId: string) => [...m, "sharedUnreadable", walletId] as const,
  sharedSession: (walletId: string, session: string) => [...m, "sharedSession", walletId, session] as const,
  sharedSessionAll: (walletId: string) => [...m, "sharedSession", walletId] as const,
  registrationKeys: (walletId: string, kind: MasternodeTypeName) => [...m, "registrationKeys", walletId, kind] as const,
};

export function useMasternodeListState() {
  const b = useBackend();
  return useQuery({ queryKey: mnKeys.listState, queryFn: () => b.mnListState() });
}

/** `retryOnMount` false: a failed read isn't read again just because this observer mounts. */
export function useMasternodes(walletId: string, enabled = true, retryOnMount = true) {
  const b = useBackend();
  return useQuery({ queryKey: mnKeys.list(walletId), queryFn: () => b.mnList(walletId), enabled, retryOnMount });
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

/** The keychain keys a registration of `kind` would use (wizard steps 3 and 4). */
export function useRegistrationKeys(walletId: string, kind: MasternodeTypeName, enabled = true) {
  const b = useBackend();
  return useQuery({ queryKey: mnKeys.registrationKeys(walletId, kind), queryFn: () => b.mnRegistrationKeys(walletId, kind), enabled, retry: false });
}

/** The keychain's public keys of `role`, one page. */
export function useMasternodeKeys(walletId: string, role: MasternodeKeyRoleName, start: number, count: number) {
  const b = useBackend();
  return useQuery({ queryKey: mnKeys.keychain(walletId, role, start, count), queryFn: () => b.mnKeys(walletId, role, start, count) });
}

/** The wallet's ProTx operations (registrations and registrar updates). */
export function useMasternodeOperations(walletId: string, enabled = true) {
  const b = useBackend();
  return useQuery({ queryKey: mnKeys.operations(walletId), queryFn: () => b.mnOperations(walletId), enabled });
}

/** A send's refusal as a sentence. `update_pending` names an operation whose state picks the line, and that state may
 * have changed while the send was out, so the operations are read again first; a failed read keeps the general line. */
export function usePendingAwareText(walletId: string, text: (e: unknown, ops?: readonly ProTxOperation[]) => string) {
  const b = useBackend();
  const queryClient = useQueryClient();
  return async (e: unknown) => {
    if (!hasCode(e, "masternodes.update_pending")) return text(e);
    const ops = await queryClient.fetchQuery({ queryKey: mnKeys.operations(walletId), queryFn: () => b.mnOperations(walletId), staleTime: 0 }).catch(() => undefined);
    return text(e, ops);
  };
}

/** What the wallet can do with shared masternodes (MG-10 §2.9). */
export function useSharedCapability(walletId: string, enabled = true) {
  const b = useBackend();
  return useQuery({ queryKey: mnKeys.sharedCapability(walletId), queryFn: () => b.mnSharedCapability(walletId), enabled });
}

/** Every shared session of the wallet: the In progress list, the badge and Auto's discovery read (§3.6). */
export function useSharedSessions(walletId: string, enabled = true) {
  const b = useBackend();
  return useQuery({ queryKey: mnKeys.sharedSessions(walletId), queryFn: () => b.mnSharedSessions(walletId), enabled });
}

/** Sessions this wallet can no longer read, listed under In progress with Discard while allowed. */
export function useSharedUnreadable(walletId: string, enabled = true) {
  const b = useBackend();
  return useQuery({ queryKey: mnKeys.sharedUnreadable(walletId), queryFn: () => b.mnSharedUnreadable(walletId), enabled });
}

/** One session; the sheet renders the record a command returns at once and lets the event re-read it. */
export function useSharedSession(walletId: string, session: string, enabled = true) {
  const b = useBackend();
  return useQuery({ queryKey: mnKeys.sharedSession(walletId, session), queryFn: () => b.mnSharedSession(walletId, session), enabled });
}

/** The wallet's masternode labels and services for governance's vote rows (MG-09 F3). */
export function useNodeNames(walletId: string): NodeNames {
  const list = useMasternodes(walletId);
  // An unlabelled row's fallback name ("Evonode b2b2…") is translated: a language change re-reads it.
  const language = useUi((s) => s.prefs.language);
  return useMemo(() => nodeNames(list.data), [list.data, language]);
}

/** Re-reads every engine-local masternode query (after a command, before its event lands). */
export function useMasternodesRefresh() {
  const client = useQueryClient();
  return () => client.invalidateQueries({ queryKey: m });
}
