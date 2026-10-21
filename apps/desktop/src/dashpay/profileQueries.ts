// My Profile's reads (DP1-10). Keys sit under `["dashpay", …]`, so every `Platform` signal
// re-reads them (events.ts), and a settled dispatch the stuck top-ups; the top-up quote sits
// under DP1-09's `quote` key, so a balance change re-reads it too (D18).

import { createContext, useContext, useEffect, useMemo } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
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
  avatarAll: ["dashpay", "avatar"] as const,
  /** DP4-03: the contract's lengths; they change with a contract version only. */
  limits: (walletId: string) => ["dashpay", "profileLimits", walletId] as const,
  /** DP4-03 D5: whether a file's picture can be uploaded (B1). */
  avatarUpload: (walletId: string) => ["dashpay", "avatarUpload", walletId] as const,
  /** DP4-03 D14: "Load contact pictures", per network; not a DashPay read (no signal moves it). */
  contactPictures: ["contactPictures"] as const,
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

/** Whose pictures the avatars show (DP4-03 D12): the window's wallet, and whether other users'
 * may be read ("Load contact pictures", D13). Outside a scope every avatar is its initials. */
export interface PictureScopeValue {
  walletId: string;
  others: boolean;
}

export const PictureScope = createContext<PictureScopeValue | null>(null);

/** "Load contact pictures" (DP4-02 D17); unread or unreadable counts as off (D13). */
export function useContactPictures() {
  const b = useBackend();
  return useQuery({ queryKey: profileKeys.contactPictures, queryFn: () => b.loadContactPictures() });
}

/** The scope App gives the window (D12); `null` until a wallet is known. */
export function usePictureScopeValue(walletId: string | undefined): PictureScopeValue | null {
  const others = useContactPictures().data === true;
  return useMemo(() => (walletId ? { walletId, others } : null), [walletId, others]);
}

/** Writes "Load contact pictures", then resets every avatar read (D14): a picture cached while on
 * goes at once, and every avatar shown reads again. */
export function useSetContactPictures() {
  const b = useBackend();
  const client = useQueryClient();
  return async (on: boolean) => {
    try {
      await b.setLoadContactPictures(on);
    } finally {
      await client.invalidateQueries({ queryKey: profileKeys.contactPictures });
      await client.resetQueries({ queryKey: profileKeys.avatarAll });
    }
  };
}

/** An identity's avatar thumbnail (DP4-02) as base64 PNG, or `null` for initials: none, a
 * picture that does not match its profile, a failed fetch, or (for another user) the toggle off,
 * when nothing is read at all (D13). `own`: one of the wallet's identities, which the engine
 * serves from its cache whatever the toggle. */
export function useAvatar(identity: string | null, size: AvatarSizeName, own = false): string | null {
  const b = useBackend();
  const scope = useContext(PictureScope);
  const enabled = scope !== null && identity !== null && (own || scope.others);
  const q = useQuery({
    queryKey: profileKeys.avatar(scope?.walletId ?? "", identity ?? "", size),
    queryFn: () => b.dashpayAvatar(scope!.walletId, identity!, size),
    enabled,
    retry: false,
    staleTime: 600_000,
  });
  return enabled ? (q.data?.png ?? null) : null;
}

export function useProfileLimits(walletId: string) {
  const b = useBackend();
  return useQuery({ queryKey: profileKeys.limits(walletId), queryFn: () => b.dashpayProfileLimits(walletId), staleTime: Infinity, retry: false });
}

export function useAvatarUploadAvailable(walletId: string) {
  const b = useBackend();
  return useQuery({ queryKey: profileKeys.avatarUpload(walletId), queryFn: () => b.dashpayAvatarUploadAvailable(walletId), staleTime: Infinity });
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
