// My Profile's reads (DP1-10). Keys sit under `["dashpay", …]`, so every `Platform` signal
// re-reads them (events.ts), and a settled dispatch the stuck top-ups; the top-up quote sits
// under DP1-09's `quote` key, so a balance change re-reads it too (D18).

import { useEffect } from "react";
import { useQuery } from "@tanstack/react-query";
import type { AvatarSizeName } from "../api/generated/AvatarSizeName";
import { keys, useBackend } from "../api/queries";
import { dashpayKeys } from "./queries";

export const profileKeys = {
  pendingTopUps: (walletId: string) => ["dashpay", "pendingTopUps", walletId] as const,
  stuckTopUpsAll: ["dashpay", "stuckTopUps"] as const,
  stuckTopUps: (walletId: string) => ["dashpay", "stuckTopUps", walletId] as const,
  contest: (walletId: string, identity: string, label: string) => ["dashpay", "contest", walletId, identity, label] as const,
  /** D12: the first page of events, apart from the bell's feed (which marks what it read). */
  outcomes: (walletId: string, identity: string) => ["dashpay", "outcomes", walletId, identity] as const,
  /** DP4-02: under `dashpay`, so a Platform signal (a profile changed) reads it again; the engine
   * answers from its cache. */
  avatar: (walletId: string, identity: string, size: AvatarSizeName) => ["dashpay", "avatar", walletId, identity, size] as const,
  /** D18 */
  topUpQuote: (walletId: string, identity: string, duffs: number) => [...dashpayKeys.quote(walletId), "topUp", identity, duffs] as const,
};

/** D12: how many events the outcome rows look through. */
const OUTCOME_EVENTS = 50;

export function useCostTable(walletId: string) {
  const b = useBackend();
  return useQuery({ queryKey: keys.costTable(walletId), queryFn: () => b.creditsCostTable(walletId), staleTime: 600_000 });
}

export function usePendingTopUps(walletId: string) {
  const b = useBackend();
  return useQuery({ queryKey: profileKeys.pendingTopUps(walletId), queryFn: () => b.creditsPendingTopUps(walletId) });
}

export function useStuckTopUps(walletId: string) {
  const b = useBackend();
  return useQuery({ queryKey: profileKeys.stuckTopUps(walletId), queryFn: () => b.creditsStuckTopUpFundings(walletId) });
}

/** The own contest (D11): read on each visit and each Platform signal, never polled. */
export function useContest(walletId: string, identity: string, label: string | null) {
  const b = useBackend();
  return useQuery({
    queryKey: profileKeys.contest(walletId, identity, label ?? ""),
    queryFn: () => b.contestStatus(walletId, identity, label!),
    enabled: label !== null,
    retry: false,
    // Votes send no wallet signal, so a visit reads them again.
    refetchOnMount: "always",
  });
}

export function useOutcomeEvents(walletId: string, identity: string) {
  const b = useBackend();
  return useQuery({ queryKey: profileKeys.outcomes(walletId, identity), queryFn: () => b.dashpayEvents(walletId, identity, null, OUTCOME_EVENTS) });
}

/** An identity's avatar thumbnail (DP4-02) as base64 PNG, or `null` for initials: none, the
 * pictures toggle off, a picture that does not match its profile, or a failed fetch. */
export function useAvatar(walletId: string, identity: string, size: AvatarSizeName): string | null {
  const b = useBackend();
  const q = useQuery({
    queryKey: profileKeys.avatar(walletId, identity, size),
    queryFn: () => b.dashpayAvatar(walletId, identity, size),
    retry: false,
    staleTime: 600_000,
  });
  return q.data?.png ?? null;
}

export function useTopUpQuote(walletId: string, identity: string | null, duffs: number | null) {
  const b = useBackend();
  return useQuery({
    queryKey: profileKeys.topUpQuote(walletId, identity ?? "", duffs ?? 0),
    queryFn: () => b.creditsTopUpQuote(walletId, identity!, duffs!),
    enabled: identity !== null && duffs !== null,
    retry: false,
  });
}

/** D9: asks Platform for the balance once per visit; the answer arrives as a Platform signal and
 * a failure shows nothing new (the engine retries in the background). */
export function useRefreshBalanceOnVisit(walletId: string, identity: string) {
  const b = useBackend();
  useEffect(() => {
    void b.identityRefreshBalance(walletId, identity).catch(() => null);
  }, [b, walletId, identity]);
}
