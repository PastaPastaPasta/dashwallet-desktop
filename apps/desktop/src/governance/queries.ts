// The Governance tab's reads and writes (MG-09). Every key sits under `["governance", …]`, so the
// engine's `governance` event re-reads them all (api/events.ts, BL-232). A new block sends no
// `governance` event (MG-02 D96), so the tip-derived reads (clock, info, the wallet's own
// proposals, whose states follow the fee's confirmations) also re-read on a timer.

import { useIsMutating, useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo } from "react";
import type { MasternodeOverview } from "../api/generated/MasternodeOverview";
import type { MyProposal } from "../api/generated/MyProposal";
import type { ProTxOperation } from "../api/generated/ProTxOperation";
import type { ProposalScopeName } from "../api/generated/ProposalScopeName";
import { errorMessage } from "../api/backend";
import { keys, useBackend } from "../api/queries";
import { useUi } from "../app/ui";
import { showToast } from "../components/toast";
import { bannedCount } from "../masternodes/model";
import { inProgress as opsInProgress } from "../masternodes/operations";
import { useMasternodeOperations, useMasternodes } from "../masternodes/queries";
import { badgeCount, GovText, governanceErrorText, inProgress, SUPERBLOCK_CHOICES } from "./model";

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

/** Whether Auto shows the Masternodes item (MG-00 D1 as amended by MG-10 Q5): a masternode of this
 * wallet or a tracked one, a collateral-shaped coin, or one of its proposals still in progress. */
export function autoShowsMasternodes(overview: MasternodeOverview | undefined, mine: MyProposal[] | undefined, ops: ProTxOperation[] | undefined): boolean {
  return (
    (overview?.masternodes.length ?? 0) > 0 ||
    (overview?.collateralCoins.length ?? 0) > 0 ||
    opsInProgress(ops ?? []).some((o) => o.kind === "registration") ||
    (mine ?? []).some(inProgress)
  );
}

/** The sidebar's Masternodes item and its badge (MG-09 D3, D5), from Settings ▸ General ▸ Show
 * Masternodes: On always shows it, Off hides it and reads nothing, Auto reads in the background and
 * shows it once the wallet holds something a masternode owner needs (`autoShowsMasternodes`);
 * `settled` is false while Auto's reads have no answer yet. */
export function useMasternodesItem(walletId: string | null): { shown: boolean; badge: number; settled: boolean } {
  const setting = useUi((s) => s.prefs.masternodesItem);
  const reads = walletId !== null && setting !== "off";
  const mine = useMyProposals(walletId ?? "", reads);
  const nodes = useMasternodes(walletId ?? "", reads);
  const ops = useMasternodeOperations(walletId ?? "", reads);
  const shown = reads && (setting === "on" || autoShowsMasternodes(nodes.data, mine.data, ops.data));
  // No answer yet: no wallet (an unlock re-reads the list), Auto's reads still out, a first read
  // that failed, or a "nothing here" being read again (a wallet switched back to after it changed).
  // A link to Masternodes then waits instead of going home.
  const answered = (q: { isPending: boolean; isError: boolean; isFetching: boolean; data: unknown }) =>
    !q.isPending && !q.isFetching && !(q.isError && q.data === undefined);
  const settled = walletId !== null && (setting !== "auto" || shown || (answered(nodes) && answered(mine) && answered(ops)));
  return { shown, badge: shown ? badgeCount(mine.data) + bannedCount(nodes.data) : 0, settled };
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

/** Platform's open contested names and the wallet's voters (MG-11). Outside the `governance`
 * prefix: the reads go to Platform, not the synced store, so a `governance` event (MG-02's
 * sync, up to once a second) doesn't re-read them; a timer, a vote and Try again do (MG-11 D3). */
export const contestedNamesKey = (walletId: string) => ["contestedNames", walletId] as const;
const NAMES_REFRESH_MS = 60_000;

export function useContestedNames(walletId: string) {
  const b = useBackend();
  return useQuery({ queryKey: contestedNamesKey(walletId), queryFn: () => b.governanceContestedNames(walletId), refetchInterval: NAMES_REFRESH_MS });
}

/** Re-reads every governance query (after a vote, a create or a submit). */
export function useGovernanceRefresh() {
  const client = useQueryClient();
  return () => client.invalidateQueries({ queryKey: g });
}
