// The Governance tab's reads and writes (MG-09). Every key sits under `["governance", …]`, so the
// engine's `governance` event re-reads them all (api/events.ts, BL-232). A new block sends no
// `governance` event (MG-02 D96), so the tip-derived reads (clock, info, the wallet's own
// proposals, whose states follow the fee's confirmations) also re-read on a timer.

import { useIsMutating, useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo } from "react";
import type { ProposalScopeName } from "../api/generated/ProposalScopeName";
import { errorMessage } from "../api/backend";
import { keys, useBackend, useUtxos } from "../api/queries";
import { useUi } from "../app/ui";
import { showToast } from "../components/toast";
import { badgeCount, GovText, governanceErrorText, masternodesShown, SUPERBLOCK_CHOICES } from "./model";

const g = keys.governanceAll;
/** Tip-derived reads: a block is about 2.5 minutes. */
const TIP_REFRESH_MS = 30_000;

const govKeys = {
  enabled: [...g, "enabled"] as const,
  syncState: [...g, "syncState"] as const,
  info: [...g, "info"] as const,
  superblocks: [...g, "superblocks"] as const,
  proposals: (scope: ProposalScopeName, text: string) => [...g, "proposals", scope, text] as const,
  voting: (walletId: string, hash: string) => [...g, "voting", walletId, hash] as const,
  mine: (walletId: string) => [...g, "mine", walletId] as const,
};

export function useGovernanceSyncState() {
  const b = useBackend();
  return useQuery({ queryKey: govKeys.syncState, queryFn: () => b.governanceSyncState() });
}

/** The clock, budget and network numbers: need only the tip (mg-engine §1). */
export function useGovernanceInfo() {
  const b = useBackend();
  return useQuery({ queryKey: govKeys.info, queryFn: () => b.governanceInfo(), refetchInterval: TIP_REFRESH_MS });
}

/** The proposals of `scope` matching `text`; only once synced (`enabled`), as they refuse before. */
export function useProposals(scope: ProposalScopeName, text: string, enabled: boolean) {
  const b = useBackend();
  return useQuery({
    queryKey: govKeys.proposals(scope, text),
    queryFn: () => b.governanceProposals({ scope, text: text === "" ? null : text }),
    enabled,
    // The rows stay while the search or scope changes (Contacts' rule, DP2-07 D15).
    placeholderData: (prev) => prev,
  });
}

/** The wallet's voting masternodes per listed proposal, with their votes seen (D8). */
export function useVotingByProposal(walletId: string, hashes: string[], enabled: boolean) {
  const b = useBackend();
  const results = useQueries({
    queries: hashes.map((hash) => ({
      queryKey: govKeys.voting(walletId, hash),
      queryFn: () => b.governanceVotingMasternodes(walletId, hash),
      enabled,
    })),
  });
  return useMemo(() => new Map(hashes.map((h, i) => [h, results[i]?.data] as const)), [hashes, results]);
}

export function useVotingMasternodes(walletId: string, hash: string) {
  const b = useBackend();
  return useQuery({ queryKey: govKeys.voting(walletId, hash), queryFn: () => b.governanceVotingMasternodes(walletId, hash) });
}

/** The wallet's own proposals: work with sync off (mg-engine §2.5). */
export function useMyProposals(walletId: string, enabled = true) {
  const b = useBackend();
  return useQuery({ queryKey: govKeys.mine(walletId), queryFn: () => b.governanceMyProposals(walletId), refetchInterval: TIP_REFRESH_MS, enabled });
}

/** The sidebar's Masternodes item (D3, D5): shown or not, and its badge. The coins are read only
 * while neither the setting nor a proposal shows it. */
export function useMasternodesItem(walletId: string | null): { shown: boolean; badge: number } {
  const setting = useUi((s) => s.prefs.showMasternodes);
  const mine = useMyProposals(walletId ?? "", walletId !== null);
  const early = masternodesShown(setting, mine.data, undefined);
  const coins = useUtxos(walletId ?? "", walletId !== null && !early && mine.isFetched);
  return {
    shown: early || masternodesShown(false, undefined, coins.data?.map((c) => c.amount)),
    badge: badgeCount(mine.data),
  };
}

export function useUpcomingSuperblocks() {
  const b = useBackend();
  return useQuery({ queryKey: govKeys.superblocks, queryFn: () => b.governanceUpcomingSuperblocks(SUPERBLOCK_CHOICES), refetchInterval: TIP_REFRESH_MS });
}

const turnOnKey = [...g, "turnOn"] as const;

/** Turns governance sync on (D6) and re-reads everything under the prefix; a refusal is a toast
 *  (D30). `busy` while any Turn on runs, so none of its buttons sends it twice. */
export function useTurnOnGovernance() {
  const b = useBackend();
  const client = useQueryClient();
  const busy = useIsMutating({ mutationKey: turnOnKey }) > 0;
  const m = useMutation({
    mutationKey: turnOnKey,
    mutationFn: () => b.governanceSetSyncEnabled(true),
    onError: (e) => showToast(`${GovText.turnOnFailed} ${governanceErrorText(e) ?? errorMessage(e)}`),
    onSettled: () => client.invalidateQueries({ queryKey: g }),
  });
  return { turnOn: () => m.mutate(), busy };
}

/** Re-reads every governance query (after a vote, a create or a submit). */
export function useGovernanceRefresh() {
  const client = useQueryClient();
  return () => client.invalidateQueries({ queryKey: g });
}
