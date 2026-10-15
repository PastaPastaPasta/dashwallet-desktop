// The DashPay status surfaces' reads (DP1-08). Keys sit under `["dashpay", …]`, so every
// `Platform` signal re-reads them (events.ts); the status and the main identity are the shell's
// (`useDashPayShell`).

import { useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import type { DashPayStatus } from "../api/generated/DashPayStatus";
import type { IdentitySummary } from "../api/generated/IdentitySummary";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import { keys, mainIdentityOf, useBackend, useLockStatus, useSyncStatus } from "../api/queries";
import { useUi } from "../app/ui";
import { useDisplay } from "../format/display";
import { isSyncing, syncPercent } from "../format/sync";
import { NAMELESS_HOLD_MS, QUOTE_REQUEST, type StatusInputs } from "./status";

export const dashpayKeys = {
  registrationsAll: ["dashpay", "registrations"] as const,
  registrations: (walletId: string) => ["dashpay", "registrations", walletId] as const,
  quote: (walletId: string) => ["dashpay", "quote", walletId] as const,
};

/** The registration a `registering` status names (S10–S14); `undefined` otherwise, while unread,
 * or when the read fails (the card then says only that a username is being created). */
export function useStatusRegistration(walletId: string | undefined, status: DashPayStatus | undefined) {
  const b = useBackend();
  const draft = status?.kind === "registering" ? status.draft : null;
  const rows = useQuery({
    queryKey: dashpayKeys.registrations(walletId ?? ""),
    queryFn: () => b.registrations(walletId!),
    enabled: !!walletId && draft !== null,
  });
  return draft === null ? undefined : rows.data?.find((r) => r.draft === draft);
}

/** D6: what a non-contested username costs this wallet (the quote's total, duffs); `null` while
 * unread or when the quote fails (J7). */
export function useUsernameCost(walletId: string, enabled: boolean): number | null {
  const b = useBackend();
  const quote = useQuery({
    queryKey: dashpayKeys.quote(walletId),
    queryFn: () => b.registrationQuote(walletId, QUOTE_REQUEST),
    enabled,
    retry: false,
  });
  return quote.data?.totalDuffs ?? null;
}

/** When each identity was first read nameless this session; a remount does not restart it. */
const namelessSince = new Map<string, number>();

/** `main`, read as still updating while it has been nameless for less than `NAMELESS_HOLD_MS`. */
function useHeldNames(main: IdentitySummary | undefined): IdentitySummary | undefined {
  const id = main?.identity;
  const nameless = !!main && !main.namesUpdating && main.mainName === null;
  const since = id && nameless ? (namelessSince.get(id) ?? Date.now()) : null;
  const [, rerender] = useState(0);
  useEffect(() => {
    if (!id) return;
    if (since === null) {
      namelessSince.delete(id);
      return;
    }
    namelessSince.set(id, since);
    const left = since + NAMELESS_HOLD_MS - Date.now();
    if (left <= 0) return;
    const timer = setTimeout(() => rerender((n) => n + 1), left);
    return () => clearTimeout(timer);
  }, [id, since]);
  return since !== null && Date.now() - since < NAMELESS_HOLD_MS ? { ...main!, namesUpdating: true } : main;
}

/** Everything the Home card and the Join page derive from (`homeCard`, `joinModel`), under the
 * shell's keys, so these reads share its cache. `retry` reads the status again (S2). */
export function useStatusInputs(walletId: string, network: NetworkInfo): { inputs: StatusInputs; retry: () => void } {
  const b = useBackend();
  const status = useQuery({ queryKey: keys.dashpayStatus(walletId), queryFn: () => b.dashpayStatus(walletId) });
  const main = mainIdentityOf(status.data);
  const identities = useQuery({ queryKey: keys.identities(walletId), queryFn: () => b.identities(walletId), enabled: main !== null });
  const mainSummary = useHeldNames(identities.data?.find((i) => i.identity === main));
  const registration = useStatusRegistration(walletId, status.data);
  const lock = useLockStatus().data?.state;
  const sync = useSyncStatus().data;
  const display = useDisplay(network);
  const discreet = useUi((s) => s.discreet);
  return {
    inputs: {
      // A failed read with an older answer cached keeps that answer (a refetch that failed).
      status: status.data,
      statusError: status.isError && status.data === undefined,
      main: mainSummary,
      registration,
      lock,
      syncPercent: sync && isSyncing(sync) ? syncPercent(sync) : null,
      date: (secs) => new Date(secs * 1000).toLocaleDateString(b.locale, { year: "numeric", month: "short", day: "numeric" }),
      amount: (duffs) => `${display.compact(duffs)} ${display.name}`,
      discreet,
    },
    retry: () => void status.refetch(),
  };
}
