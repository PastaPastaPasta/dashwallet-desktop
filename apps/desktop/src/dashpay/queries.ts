// The DashPay status surfaces' reads (DP1-08). Keys sit under `["dashpay", …]`, so every
// `Platform` signal re-reads them (events.ts); the status and the main identity are the shell's
// (`useDashPayShell`).

import { useEffect, useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import type { Backend } from "../api/backend";
import type { DashPayStatus } from "../api/generated/DashPayStatus";
import type { NameAvailability } from "../api/generated/NameAvailability";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { RegistrationQuote } from "../api/generated/RegistrationQuote";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import { useDebounced } from "../app/useDebounced";
import { keys, mainIdentityOf, mainSummaryOf, useBackend, useLockStatus, useSyncStatus, useWallets } from "../api/queries";
import { useUi } from "../app/ui";
import { useDisplay } from "../format/display";
import { useModel } from "../onboarding/model";
import { isSyncing, syncPercent } from "../format/sync";
import type { PaySource, PayWithInputs } from "./payWith";
import { AVAILABILITY_DEBOUNCE_MS, isRunning } from "./registration";
import { AvailabilityChecker, needsAvailability, suggest } from "./instantName";
import { contestRow, lacksTemporary, registrationOffered, rereadAfterSecond, rowSecond, temporaryOutcome, type InstantInputs, type NamesRead } from "./instantOffer";
import { QUOTE_REQUEST, READINESS_FAILED, type StatusInputs } from "./status";
import { coreRequest, invitationRequest, platformRequest, shieldedRequest } from "./wizard";

export const dashpayKeys = {
  registrationsAll: ["dashpay", "registrations"] as const,
  registrations: (walletId: string) => ["dashpay", "registrations", walletId] as const,
  quote: (walletId: string) => ["dashpay", "quote", walletId] as const,
  /** DP1-09: the quote of one label, under `quote`, so a balance change re-reads it too. */
  labelQuote: (walletId: string, label: string, source: PaySource = "core", temporaryLabel: string | null = null) => {
    const key = source === "core" ? (["dashpay", "quote", walletId, label] as const) : (["dashpay", "quote", walletId, label, source] as const);
    // An instant username beside it is priced too (D4 reversed).
    return temporaryLabel === null ? key : ([...key, { temporaryLabel }] as const);
  },
  availability: (walletId: string, label: string) => ["dashpay", "availability", walletId, label] as const,
  nameCredits: (walletId: string, identity: string, label: string) => ["dashpay", "nameCredits", walletId, identity, label] as const,
  /** Pure and offline (`check_username`): no signal changes it, so it is not under `dashpay`. */
  usernameCheck: (label: string) => ["usernameCheck", label] as const,
  /** Tools ▸ Information's DashPay card (DP6-03); sync events re-read every wallet's (D6). */
  syncInfoAll: ["dashpay", "syncInfo"] as const,
  syncInfo: (walletId: string) => ["dashpay", "syncInfo", walletId] as const,
  /** DP5-03: a stashed invitation's status (`invitation_status`), and a claim's quote. */
  invitation: (linkId: string) => ["dashpay", "invitation", linkId] as const,
  claimQuote: (walletId: string, label: string, linkId: string, requestInviter: boolean) =>
    ["dashpay", "quote", walletId, label, "invitation", linkId, requestInviter] as const,
  /** C-funding F16: the Shielded balance's readiness to pay a registration, under `quote`; an
   * object where `labelQuote` has its label, so no username shares its key. */
  shieldedReadiness: (walletId: string) => ["dashpay", "quote", walletId, { funding: "shielded" }] as const,
  /** The identities read with the time it started, for what became of a temporary name. */
  namesRead: (walletId: string, temporaryLabel: string | null, second: number | null) => ["dashpay", "namesRead", walletId, temporaryLabel, second] as const,
  /** C-funding F19: the "Pay with" step's Platform quote of a plain name, keyed as the readiness. */
  platformSource: (walletId: string) => ["dashpay", "quote", walletId, { funding: "platform" }] as const,
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

/** The instant form's reads (D4 reversed): the offline checklist of `candidate` at once, its
 * availability only once it passed the rules, is not contested and is not `requested`, and the
 * typing settled; a stale answer is dropped. */
export function useInstantChecks(walletId: string, requested: string, candidate: string) {
  const b = useBackend();
  const check = useUsernameCheck(candidate).data;
  const checker = useMemo(() => new AvailabilityChecker((label) => b.nameAvailability(walletId, label)), [b, walletId]);
  useModel(checker);
  useEffect(() => () => checker.dispose(), [checker]);
  const eligible = needsAvailability(requested, check) ? candidate : null;
  useEffect(() => checker.set(eligible), [checker, eligible]);
  const current = eligible !== null && checker.label === eligible;
  return { check, availability: current ? checker.availability : undefined, pending: eligible !== null && (!current || checker.pending) };
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

/** Step 2's quote for `label` and the instant username beside it, from the Dash wallet or the
 * Platform balance. */
export function useLabelQuote(walletId: string, label: string, enabled: boolean, source: PaySource = "core", temporaryLabel: string | null = null) {
  const b = useBackend();
  const request = { core: coreRequest, platform: (l: string, temp: string | null) => platformRequest(l, undefined, temp), shielded: (l: string, temp: string | null) => shieldedRequest(l, undefined, temp) }[source];
  return useQuery({
    queryKey: dashpayKeys.labelQuote(walletId, label, source, temporaryLabel),
    queryFn: () => b.registrationQuote(walletId, request(label, temporaryLabel)),
    enabled,
    retry: false,
  });
}

/** DP5-03 C4–C9: what the network says of the stashed invitation `linkId`. */
export function useInvitationStatus(linkId: string, enabled: boolean) {
  const b = useBackend();
  return useQuery({ queryKey: dashpayKeys.invitation(linkId), queryFn: () => b.invitationStatus(linkId), enabled, retry: false });
}

/** DP5-03 C11: the claim's quote for `label`: its grant is the start's (D9). */
export function useClaimQuote(walletId: string, label: string, linkId: string, requestInviter: boolean) {
  const b = useBackend();
  return useQuery({
    queryKey: dashpayKeys.claimQuote(walletId, label, linkId, requestInviter),
    queryFn: () => b.registrationQuote(walletId, invitationRequest(label, linkId, requestInviter)),
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

/** The "Pay with" step's Shielded row (C-funding F16): the quote's readiness, for the label the
 * Join page quotes (the denomination covers a plain name; the engine ignores the request's). */
export function useShieldedReadiness(walletId: string, enabled = true) {
  const b = useBackend();
  const quote = useQuery({
    queryKey: dashpayKeys.shieldedReadiness(walletId),
    queryFn: () => b.registrationQuote(walletId, shieldedRequest(QUOTE_REQUEST.label)),
    enabled,
    retry: false,
  });
  return quote.isError ? READINESS_FAILED : (quote.data?.readiness ?? null);
}

/** The "Pay with" step's Platform row (C-funding F19): a plain name's quote from the Platform
 * balance, or its refusal, which is the row's reason; `undefined` while unread. */
export function usePlatformSource(walletId: string) {
  const b = useBackend();
  const quote = useQuery({
    queryKey: dashpayKeys.platformSource(walletId),
    queryFn: () => b.registrationQuote(walletId, platformRequest(QUOTE_REQUEST.label)),
    retry: false,
  });
  return platformSourceOf(quote);
}

/** The row's read: a refetch that fails keeps the last data, so its refusal comes first. */
export const platformSourceOf = (q: { data: RegistrationQuote | undefined; isError: boolean; error: unknown }): PayWithInputs["platform"] =>
  q.isError ? { error: q.error } : q.data ? { quote: q.data } : undefined;

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

/** The names read for `row`'s temporary name: the identities with when the read started, and
 * DPNS's answer for the label once they lack it (`temporaryOutcome`). Keyed by the row's second,
 * so the row reaching its contest reads afresh: a read from before says nothing. */
export function namesReadQuery(b: Backend, walletId: string, row: RegistrationStatus | undefined) {
  const temporaryLabel = row?.temporaryLabel ?? null;
  return {
    queryKey: dashpayKeys.namesRead(walletId, temporaryLabel, rowSecond(row)),
    queryFn: async (): Promise<NamesRead> => {
      const read: NamesRead = { startedAt: Date.now(), identities: await b.identities(walletId) };
      if (temporaryLabel === null || !lacksTemporary(row, read)) return read;
      const unknown: NameAvailability = { kind: "unknown" };
      return { ...read, temporary: await b.nameAvailability(walletId, temporaryLabel).catch(() => unknown) };
    },
    enabled: temporaryLabel !== null,
    retry: false,
  };
}

/** A failed read says nothing, not even what an earlier one said. */
export const answeredRead = (read: { isError: boolean; data?: NamesRead }): NamesRead | undefined => (read.isError ? undefined : read.data);

/** The identities as one read answered, with when it started: a failed read says nothing. */
export function useNamesRead(walletId: string, row: RegistrationStatus | undefined): NamesRead | undefined {
  const b = useBackend();
  const read = useQuery(namesReadQuery(b, walletId, row));
  const second = rowSecond(row);
  const startedAt = read.data?.startedAt;
  const { refetch } = read;
  useEffect(() => rereadAfterSecond(second, startedAt, () => void refetch()), [second, startedAt, refetch]);
  return answeredRead(read);
}

/** What the voting card and the request's details offer while the wallet's own contest is open
 * (D4 reversed): the Join page's gate, a plain name's credits, and the temporary name its
 * registration asked for. `undefined` with no open contest. */
export function useInstantInputs(walletId: string, status: DashPayStatus | undefined): InstantInputs | undefined {
  const contest = status?.kind === "contestPending" ? status : null;
  const wallets = useWallets();
  const rows = useRegistrations(walletId, contest !== null);
  const row = contest ? contestRow(rows.data, contest.identity, contest.label) : undefined;
  const read = useNamesRead(walletId, row);
  const outcome = temporaryOutcome(row, read);
  // The credits matter only while the card may offer a name: a new instant username, or the
  // registration's temporary name again.
  const label = row?.temporaryLabel ?? (contest ? suggest(contest.label) : "");
  const mayOffer = contest !== null && rows.data !== undefined && (row === undefined || outcome === "failed");
  const credits = useNameCredits(walletId, mayOffer ? contest.identity : null, label);
  if (!contest) return undefined;
  const wallet = wallets.data?.find((w) => w.walletId === walletId);
  return {
    offered: registrationOffered(wallet),
    nameCredits: credits.data?.maxCredits ?? null,
    asked: rows.data === undefined ? undefined : row?.temporaryLabel ? {
      label: row.temporaryLabel,
      outcome,
      recorded: row.temporaryNameOutcome !== undefined,
    } : null,
  };
}

/** Everything the Home card and the Join page derive from (`homeCard`, `joinModel`), under the
 * shell's keys, so these reads share its cache. `retry` reads the status again (S2). */
export function useStatusInputs(walletId: string, network: NetworkInfo): { inputs: StatusInputs; retry: () => void } {
  const b = useBackend();
  const status = useQuery({ queryKey: keys.dashpayStatus(walletId), queryFn: () => b.dashpayStatus(walletId) });
  const main = mainIdentityOf(status.data);
  const identities = useQuery({ queryKey: keys.identities(walletId), queryFn: () => b.identities(walletId), enabled: main !== null });
  const registration = useStatusRegistration(walletId, status.data);
  const instant = useInstantInputs(walletId, status.data);
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
      instant,
    },
    retry: () => void status.refetch(),
  };
}
