// The DashPay status surfaces' reads (DP1-08). Keys sit under `["dashpay", …]`, so every
// `Platform` signal re-reads them (events.ts); the status and the main identity are the shell's
// (`useDashPayShell`).

import { useQuery } from "@tanstack/react-query";
import type { DashPayStatus } from "../api/generated/DashPayStatus";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import { useDebounced } from "../app/useDebounced";
import { keys, mainIdentityOf, mainSummaryOf, useBackend, useLockStatus, useSyncStatus } from "../api/queries";
import { useUi } from "../app/ui";
import { useDisplay } from "../format/display";
import { isSyncing, syncPercent } from "../format/sync";
import { AVAILABILITY_DEBOUNCE_MS, isRunning } from "./registration";
import { QUOTE_REQUEST, type StatusInputs } from "./status";
import { coreRequest } from "./wizard";

export const dashpayKeys = {
  registrationsAll: ["dashpay", "registrations"] as const,
  registrations: (walletId: string) => ["dashpay", "registrations", walletId] as const,
  quote: (walletId: string) => ["dashpay", "quote", walletId] as const,
  /** DP1-09: the quote of one label, under `quote`, so a balance change re-reads it too. */
  labelQuote: (walletId: string, label: string) => ["dashpay", "quote", walletId, label] as const,
  availability: (walletId: string, label: string) => ["dashpay", "availability", walletId, label] as const,
  nameCredits: (walletId: string, identity: string, label: string) => ["dashpay", "nameCredits", walletId, identity, label] as const,
  /** Pure and offline (`check_username`): no signal changes it, so it is not under `dashpay`. */
  usernameCheck: (label: string) => ["usernameCheck", label] as const,
  /** Tools ▸ Information's DashPay card (DP6-03). */
  syncInfo: (walletId: string) => ["dashpay", "syncInfo", walletId] as const,
};

/** DP1-09 D15: a running registration's `waiting` changes without a Platform signal, so its
 * rows are read again this often while one runs. */
export const REGISTRATION_POLL_MS = 5_000;

/** The wallet's registrations, read again while one runs (D15). */
export function useRegistrations(walletId: string, enabled = true) {
  const b = useBackend();
  return useQuery({
    queryKey: dashpayKeys.registrations(walletId),
    queryFn: () => b.registrations(walletId),
    enabled,
    refetchInterval: (q) => (q.state.data?.some(isRunning) ? REGISTRATION_POLL_MS : false),
  });
}

/** Step 1's reads (F4): the offline checklist at once, the network's availability once the
 * typing settled and the label is valid. `pending` until the availability answers for the label
 * as typed now. */
export function useNameChecks(walletId: string, label: string) {
  const b = useBackend();
  const settled = useDebounced(label, AVAILABILITY_DEBOUNCE_MS);
  const check = useUsernameCheck(label);
  const availability = useQuery({
    queryKey: dashpayKeys.availability(walletId, settled),
    queryFn: () => b.nameAvailability(walletId, settled),
    enabled: settled === label && check.data?.valid === true,
    retry: false,
  });
  // A failed lookup is "could not check": the start decides (D3). While the typing settles the
  // answer is `pending`, which every reader checks first.
  const answer = availability.isError ? ({ kind: "unknown" } as const) : availability.data;
  return { check: check.data, availability: answer, pending: settled !== label || availability.isFetching };
}

/** The engine's offline checklist for `label` (`check_username`). */
export function useUsernameCheck(label: string) {
  const b = useBackend();
  return useQuery({ queryKey: dashpayKeys.usernameCheck(label), queryFn: () => b.checkUsername(label), enabled: label !== "", staleTime: Infinity });
}

/** D6: the credits a name for `identity` may cost (`grant_request(registerName)`), for step 2. */
export function useNameCredits(walletId: string, identity: string | null, label: string) {
  const b = useBackend();
  return useQuery({
    queryKey: dashpayKeys.nameCredits(walletId, identity ?? "", label),
    queryFn: () => b.grantRequest(walletId, identity!, { kind: "registerName", label }),
    enabled: identity !== null,
    retry: false,
  });
}

/** Step 2's quote for `label` (Core funding). */
export function useLabelQuote(walletId: string, label: string, enabled: boolean) {
  const b = useBackend();
  return useQuery({
    queryKey: dashpayKeys.labelQuote(walletId, label),
    queryFn: () => b.registrationQuote(walletId, coreRequest(label)),
    enabled,
    retry: false,
  });
}

/** The registration a `registering` status names (S10–S14); `undefined` otherwise, while unread,
 * or when the read fails (the card then says only that a username is being created). */
export function useStatusRegistration(walletId: string | undefined, status: DashPayStatus | undefined) {
  const draft = status?.kind === "registering" ? status.draft : null;
  const rows = useRegistrations(walletId ?? "", !!walletId && draft !== null);
  return draft === null ? undefined : rows.data?.find((r) => r.draft === draft);
}

/** D6: the quote of a non-contested username for this wallet: its total is what one costs, its
 * `availableDuffs` what the funding can spend now (DP1-09 D5); `undefined` while unread or when
 * the quote fails (J7). */
export function useUsernameQuote(walletId: string, enabled: boolean) {
  const b = useBackend();
  const quote = useQuery({
    queryKey: dashpayKeys.quote(walletId),
    queryFn: () => b.registrationQuote(walletId, QUOTE_REQUEST),
    enabled,
    retry: false,
  });
  return quote.data;
}

/** Everything the Home card and the Join page derive from (`homeCard`, `joinModel`), under the
 * shell's keys, so these reads share its cache. `retry` reads the status again (S2). */
export function useStatusInputs(walletId: string, network: NetworkInfo): { inputs: StatusInputs; retry: () => void } {
  const b = useBackend();
  const status = useQuery({ queryKey: keys.dashpayStatus(walletId), queryFn: () => b.dashpayStatus(walletId) });
  const main = mainIdentityOf(status.data);
  const identities = useQuery({ queryKey: keys.identities(walletId), queryFn: () => b.identities(walletId), enabled: main !== null });
  const registration = useStatusRegistration(walletId, status.data);
  const lock = useLockStatus().data?.state;
  const sync = useSyncStatus().data;
  const display = useDisplay(network);
  const discreet = useUi((s) => s.discreet);
  return {
    inputs: {
      status: status.data,
      // A refetch that failed is an error too: the answer cached before it may be wrong now (D24).
      statusError: status.isError,
      main: mainSummaryOf(identities, main),
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
