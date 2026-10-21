// Tools ▸ Information's DashPay card (DP6-03, DASHPAY F23, iOS Sync Info): the `DashPaySyncInfoViewModel`
// of DASHPAY §4 over `dashpay_sync_status` (the bring-up's verdict, the last pass, contacts waiting for
// an unlock, the Platform loops, the quorum source) and BL-153's `dashpay_status`, which says whether
// the wallet has an identity. Plain TypeScript, no React; the screen is screens/tools/DashPayInfo.tsx.

import type { DashPayStatus } from "../api/generated/DashPayStatus";
import type { DashPaySyncStatus } from "../api/generated/DashPaySyncStatus";
import type { QuorumSource } from "../api/generated/QuorumSource";
import type { StartupStatus } from "../api/generated/StartupStatus";
import type { SyncLoop } from "../api/generated/SyncLoop";
import type { SyncLoopStatus } from "../api/generated/SyncLoopStatus";
import type { SyncPassReport } from "../api/generated/SyncPassReport";
import { COPY } from "../dashpay/status";
import { dateText, NONE, type InfoRow } from "./information";

/** The card's states (design note §2), in the order `dashpayInfo` decides them. */
export type DashPayInfoState =
  | "noWallet"
  | "error"
  | "loading"
  | "watchOnly"
  | "failed"
  | "notStarted"
  | "starting"
  | "notSetUp"
  | "stopped"
  | "degraded"
  | "running";

/** The chip's tone (its `chip-*` class): `success` running on SPV-verified quorum keys, `warning`
 * degraded, `danger` failed or unread, `neutral` the rest. */
export type DashPayInfoTone = "success" | "neutral" | "warning" | "danger";

export interface DashPayInfo {
  state: DashPayInfoState;
  tone: DashPayInfoTone;
  /** The chip beside the headline. */
  chip: string;
  headline: string;
  line: string | null;
  /** A spinner beside the headline: DashPay is busy and nothing waits for the user. */
  busy: boolean;
  rows: InfoRow[];
  /** Sync now (`dashpay_sync_soon`, D3); `null` where the card offers none. */
  syncNow: { disabled: boolean; note: string | null } | null;
}

export type TrustedOnlyRead = boolean | "failed" | undefined;

export interface DashPayInfoInputs {
  /** No wallet on this network: only the session's quorum source is read. */
  noWallet: boolean;
  /** The wallet's summary says it has no keys (`WalletSummary.watchOnly`). */
  watchOnly: boolean;
  /** BL-153's status (the shell's query), `undefined` while unread or when it failed. */
  status: DashPayStatus | undefined;
  sync: DashPaySyncStatus | undefined;
  /** Why the sync status (or the wallet list) could not be read, else `null`. */
  syncError: string | null;
  /** `quorum_source` (no wallet) or the sync status's own. */
  quorumSource: QuorumSource | undefined;
  /** The developer toggle (`trusted_quorums_only`): `undefined` while unread (the default, the
   * degraded mode, is assumed), `"failed"` when the read failed (neither is claimed, D5). */
  trustedOnly: TrustedOnlyRead;
  /** UNIX seconds. */
  now: number;
  timeZone?: string;
}

export const INFO_COPY = {
  noWallet: "No wallet on this network yet",
  error: "Could not read DashPay's sync state",
  loading: "Reading DashPay's sync state",
  passFailed: { title: "The last DashPay pass failed", message: (code: string) => `It stopped with ${code}.` },
  notSetUp: { title: "No DashPay identity on this wallet", message: "Join DashPay to create a username." },
  notSetUpWaiting: "Join DashPay once the wallet has synced.",
  stopped: "DashPay sync is stopped",
  startingUp: "DashPay is starting",
  degraded: {
    trusted: "Quorum keys from a trusted service",
    trustedFallback: "Trusted quorum keys until masternode sync finishes",
    trustedOnly: "SPV quorum checks are off (developer setting)",
  },
  running: "Quorum keys verified by SPV",
  syncNow: "Sync now",
  loopsStopped: "DashPay's sync loops are not running. They run while the wallet is connected to the Dash network, and can take a little while to start after it connects.",
  watchOnly: "A watch-only wallet can't use DashPay because it has no keys.",
  // n7: a pass syncs a known identity's contacts and payments; it does not look for one.
  heldNoIdentityYet: "Sync now waits until DashPay finds this wallet's identity.",
  heldNotSetUp: "Sync now waits until this wallet has a DashPay identity.",
  heldStartingUp: "Sync now waits until DashPay has started.",
  requested: "A DashPay pass was asked for. It runs in the background. Passes don't report their results yet, so this card can't show when it ends.",
  pendingContactsNote: "They're built after an unlock or the DIP-15 contact rescan.",
} as const;

const CHIP: Record<DashPayInfoState, string> = {
  noWallet: "No wallet",
  error: "Unread",
  loading: "Reading",
  watchOnly: "Watch-only",
  failed: "Failed",
  notStarted: "Not started",
  starting: "Starting",
  notSetUp: "Not set up",
  stopped: "Stopped",
  degraded: "Degraded",
  running: "Running",
};

/** The bring-up outcomes that are failures while no identity is known, as the Home card reads them
 * (status.ts `startupCard`). */
const FAILED: ReadonlySet<StartupStatus> = new Set(["discoveryFailed", "identityScanIncomplete", "partialNoIdentity", "partialAccountsPending", "seedBindingUnverified"]);

/** The scan verdict: the bring-up's outcome (DASHPAY §3.2 "Startup status values"). */
export const STARTUP_TEXT: Record<StartupStatus, { value: string; note?: string }> = {
  // Also a watch-only wallet's, and a wallet's that Platform showed has no identity (bringup.rs).
  notRun: { value: "Not run" },
  starting: { value: "Looking for identities…" },
  ready: { value: "Complete: identity found" },
  noIdentity: { value: "Complete: no identity" },
  partialNoIdentity: { value: "Incomplete: no identity found so far", note: "It looks again the next time the wallet starts." },
  discoveryFailed: { value: "Failed: discovery did not finish", note: "It looks again the next time the wallet starts." },
  partialAccountsPending: { value: "Incomplete: contact accounts wait for an unlock" },
  seedBindingUnverified: { value: "Failed: keys not confirmed", note: COPY.unverified.message },
  identityScanIncomplete: { value: "Incomplete: the identity scan did not finish", note: "It looks again the next time the wallet starts." },
  identityUnsettled: { value: "Waiting for an unlock", note: COPY.unlockToLook.message },
};

const LOOP_TITLE: Record<SyncLoop, string> = {
  identitySync: "Identities loop",
  dashPaySync: "Contacts and payments loop",
  dpnsSync: "Username contests loop",
  platformAddressSync: "Platform addresses loop",
};

/** D5: where Platform proofs get their quorum keys, and what that means for what the card shows. */
export function quorumRow(source: QuorumSource | undefined, trustedOnly: TrustedOnlyRead): InfoRow {
  const title = "Quorum keys";
  switch (source) {
    case undefined:
      return { title, value: NONE };
    case "spv":
      return { title, value: "SPV (verified masternode list)" };
    case "trustedFallback":
      return { title, value: "Trusted service until masternode sync finishes", note: "Platform data read meanwhile counts as unverified." };
    case "trusted":
      if (trustedOnly === "failed") return { title, value: "Trusted service", note: "Could not read whether SPV checks of quorum keys are off." };
      return trustedOnly === true
        ? { title, value: "Trusted service only (developer setting)", note: "SPV checks of quorum keys are off." }
        : { title, value: "Trusted service (degraded mode)", note: "Checked against SPV's quorum keys where SPV holds them." };
  }
}

/** A time of today as "11:59:45", an older one as Information's dates. */
export function clockText(secs: number, now: number, timeZone?: string): string {
  const day = (s: number) => new Intl.DateTimeFormat("en-US", { dateStyle: "short", timeZone }).format(new Date(s * 1000));
  if (day(secs) !== day(now)) return dateText(secs, timeZone);
  return new Intl.DateTimeFormat("en-US", { hour: "2-digit", minute: "2-digit", second: "2-digit", hourCycle: "h23", timeZone }).format(new Date(secs * 1000));
}

export function loopRow(loop: SyncLoopStatus, now: number, timeZone?: string): InfoRow {
  const clock = (s: number) => clockText(s, now, timeZone);
  const title = LOOP_TITLE[loop.syncLoop];
  if (!loop.running) return { title, value: loop.lastRunAt === null ? "Stopped" : `Stopped · last run ${clock(loop.lastRunAt)}` };
  if (loop.lastRunAt === null) return { title, value: "Running · no run yet" };
  const next = loop.nextRunAt === null ? "" : ` · next ${clock(loop.nextRunAt)}`;
  return { title, value: `Running · last run ${clock(loop.lastRunAt)}${next}` };
}

/** `contact_accounts_pending`: contact accounts waiting for an unlock or the DIP-15 rescan
 * (startup_status.rs). */
const PENDING_TITLE = "Contact accounts waiting to be built";

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** D4: the last pass's end and what it found; "—" until a pass reports (`last_pass` is `None` today). */
export function lastPassRow(pass: SyncPassReport | null, timeZone?: string): InfoRow {
  const title = "Last pass";
  if (!pass) return { title, value: NONE, note: "No pass has reported yet." };
  const value = dateText(pass.finishedAt, timeZone);
  if (pass.failureCode !== null) return { title, value, note: `Failed: ${pass.failureCode}` };
  const found = [
    pass.newRequests && plural(pass.newRequests, "new request", "new requests"),
    pass.newContacts && plural(pass.newContacts, "new contact", "new contacts"),
    pass.newPayments && plural(pass.newPayments, "new payment", "new payments"),
  ].filter(Boolean);
  return { title, value, note: found.length ? found.join(", ") : "Nothing new" };
}

function degradedHeadline(source: QuorumSource, trustedOnly: TrustedOnlyRead): string {
  if (source === "trustedFallback") return INFO_COPY.degraded.trustedFallback;
  return trustedOnly === true ? INFO_COPY.degraded.trustedOnly : INFO_COPY.degraded.trusted;
}

const held = (note: string) => ({ disabled: true, note });

function card(state: DashPayInfoState, tone: DashPayInfoTone, headline: string, rest: Partial<DashPayInfo> = {}): DashPayInfo {
  return { state, tone, chip: CHIP[state], headline, line: null, busy: false, rows: [], syncNow: null, ...rest };
}

/** The card for the wallet's DashPay sync state (design note §2, D2): the first that applies of no
 * wallet, unread, watch-only, a failed pass, the bring-up's own state while no identity is known
 * (failed, not run, running), no identity, the bring-up running with the loops not yet started,
 * the DashPay loop stopped, quorum keys not from SPV, running. Sync now is held, with the reason,
 * wherever it would do nothing (D3). */
export function dashpayInfo(i: DashPayInfoInputs): DashPayInfo {
  const quorum = quorumRow(i.sync?.quorumSource ?? i.quorumSource, i.trustedOnly);
  if (i.noWallet) return card("noWallet", "neutral", INFO_COPY.noWallet, { rows: [quorum] });
  const s = i.sync;
  if (!s) {
    if (i.syncError === null) return card("loading", "neutral", INFO_COPY.loading, { busy: true });
    return card("error", "danger", INFO_COPY.error, { line: i.syncError });
  }
  const verdict = STARTUP_TEXT[s.startup];
  const rows: InfoRow[] = [
    { title: "Identity scan", ...verdict },
    lastPassRow(s.lastPass, i.timeZone),
    s.pendingContactCrypto === 0
      ? { title: PENDING_TITLE, value: "None" }
      : { title: PENDING_TITLE, value: String(s.pendingContactCrypto), note: INFO_COPY.pendingContactsNote },
    ...s.loops.map((l) => loopRow(l, i.now, i.timeZone)),
    quorum,
  ];
  // D3: the pass Sync now asks for runs only while the DashPay loop does.
  const looping = s.loops.some((l) => l.syncLoop === "dashPaySync" && l.running);
  const syncNow = looping ? { disabled: false, note: null } : held(INFO_COPY.loopsStopped);
  const at = (state: DashPayInfoState, tone: DashPayInfoTone, headline: string, rest: Partial<DashPayInfo> = {}) =>
    card(state, tone, headline, { rows, syncNow, ...rest });
  const { status } = i;
  // An identity on file settles the bring-up's outcome (BL-153 D2), and a settled absence too. With
  // the status unread the outcome stands in for an identity, never for an absence: the bring-up's
  // `noIdentity` proves none (BL-153 D16, D2).
  const identity = status ? status.kind === "ready" || status.kind === "registering" || status.kind === "contestPending" : s.startup === "ready";
  const absent = status?.kind === "noIdentity";

  if (i.watchOnly || (status?.kind === "noIdentity" && status.reason === "watchOnly"))
    return at("watchOnly", "neutral", COPY.watchOnly.title, { syncNow: held(INFO_COPY.watchOnly) });
  if (s.lastPass?.failureCode) return at("failed", "danger", INFO_COPY.passFailed.title, { line: INFO_COPY.passFailed.message(s.lastPass.failureCode) });
  if (!identity && !absent) {
    const look = (state: DashPayInfoState, tone: DashPayInfoTone, headline: string, rest: Partial<DashPayInfo> = {}) =>
      at(state, tone, headline, { syncNow: held(INFO_COPY.heldNoIdentityYet), ...rest });
    if (s.startup === "seedBindingUnverified") return look("failed", "danger", COPY.unverified.title, { line: COPY.unverified.message });
    if (FAILED.has(s.startup)) return look("failed", "danger", COPY.lookFailed.title, { line: COPY.lookFailed.message });
    if (s.startup === "notRun") return look("notStarted", "neutral", COPY.notStarted.title, { line: COPY.notStarted.message });
    if (s.startup === "identityUnsettled") return look("starting", "neutral", COPY.unlockToLook.title, { line: COPY.unlockToLook.message });
    // A bring-up running, or (BL-153 D16) an outcome the status has not settled on yet.
    return look("starting", "neutral", COPY.looking.title, { busy: true });
  }
  if (absent)
    return at("notSetUp", "neutral", INFO_COPY.notSetUp.title, {
      line: status?.kind === "noIdentity" && status.reason === "waitingForSync" ? INFO_COPY.notSetUpWaiting : INFO_COPY.notSetUp.message,
      syncNow: held(INFO_COPY.heldNotSetUp),
    });
  // D19: every launch runs the bring-ups before SPV and the loops start (bringup.rs `supervise`).
  if (!looping && s.startup === "starting") return at("starting", "neutral", INFO_COPY.startingUp, { busy: true, syncNow: held(INFO_COPY.heldStartingUp) });
  // Sync now's note says why.
  if (!looping) return at("stopped", "neutral", INFO_COPY.stopped);
  if (s.quorumSource !== "spv") return at("degraded", "warning", degradedHeadline(s.quorumSource, i.trustedOnly));
  return at("running", "success", INFO_COPY.running);
}
