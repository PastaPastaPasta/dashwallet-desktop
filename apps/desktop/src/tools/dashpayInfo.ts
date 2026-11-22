// Tools ▸ Information's DashPay card (DP6-03, DASHPAY F23, iOS Sync Info): the `DashPaySyncInfoViewModel`
// of DASHPAY §4 over `dashpay_sync_status` (the bring-up's verdict, the last pass, contact accounts
// waiting to be built, the Platform loops, the quorum source) and BL-153's `dashpay_status`, which says whether
// the wallet has an identity. Plain TypeScript, no React; the screen is screens/tools/DashPayInfo.tsx.

import type { DashPayStatus } from "../api/generated/DashPayStatus";
import type { DashPaySyncStatus } from "../api/generated/DashPaySyncStatus";
import type { QuorumSource } from "../api/generated/QuorumSource";
import type { StartupStatus } from "../api/generated/StartupStatus";
import type { SyncLoop } from "../api/generated/SyncLoop";
import type { SyncLoopStatus } from "../api/generated/SyncLoopStatus";
import type { SyncPassReport } from "../api/generated/SyncPassReport";
import { COPY } from "../dashpay/status";
import { messageTable, t } from "../i18n";
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
  get noWallet() {
    return t("tools.dashpayInfo.noWallet");
  },
  get error() {
    return t("tools.dashpayInfo.error");
  },
  get loading() {
    return t("tools.dashpayInfo.loading");
  },
  passFailed: {
    get title() {
      return t("tools.dashpayInfo.passFailedTitle");
    },
    message: (code: string) => t("tools.dashpayInfo.passFailedMessage", { code }),
  },
  notSetUp: {
    get title() {
      return t("tools.dashpayInfo.notSetUpTitle");
    },
    get message() {
      return t("tools.dashpayInfo.notSetUpMessage");
    },
  },
  get notSetUpWaiting() {
    return t("tools.dashpayInfo.notSetUpWaiting");
  },
  get stopped() {
    return t("tools.dashpayInfo.stopped");
  },
  get startingUp() {
    return t("tools.dashpayInfo.startingUp");
  },
  degraded: messageTable("tools.dashpayInfo.degraded", ["trusted", "trustedFallback", "trustedOnly"]),
  get running() {
    return t("tools.dashpayInfo.running");
  },
  get syncNow() {
    return t("tools.dashpayInfo.syncNow");
  },
  get loopsStopped() {
    return t("tools.dashpayInfo.loopsStopped");
  },
  get watchOnly() {
    return t("tools.dashpayInfo.watchOnly");
  },
  // n7: a pass syncs a known identity's contacts and payments; it does not look for one.
  get heldNoIdentityYet() {
    return t("tools.dashpayInfo.heldNoIdentityYet");
  },
  get heldNotSetUp() {
    return t("tools.dashpayInfo.heldNotSetUp");
  },
  get heldStartingUp() {
    return t("tools.dashpayInfo.heldStartingUp");
  },
  get requested() {
    return t("tools.dashpayInfo.requested");
  },
  get pendingContactsNote() {
    return t("tools.dashpayInfo.pendingContactsNote");
  },
} as const;

const CHIP: Readonly<Record<DashPayInfoState, string>> = messageTable("tools.dashpayInfo.chip", [
  "noWallet",
  "error",
  "loading",
  "watchOnly",
  "failed",
  "notStarted",
  "starting",
  "notSetUp",
  "stopped",
  "degraded",
  "running",
]);

/** The bring-up outcomes that are failures while no identity is known, as the Home card reads them
 * (status.ts `startupCard`). */
const FAILED: ReadonlySet<StartupStatus> = new Set(["discoveryFailed", "identityScanIncomplete", "partialNoIdentity", "partialAccountsPending", "seedBindingUnverified"]);

/** The scan verdict: the bring-up's outcome (DASHPAY §3.2 "Startup status values"). */
export const STARTUP_TEXT: Readonly<Record<StartupStatus, { value: string; note?: string }>> = {
  // Also a watch-only wallet's, and a wallet's that Platform showed has no identity (bringup.rs).
  get notRun() {
    return { value: t("tools.dashpayInfo.startup.notRun") };
  },
  get starting() {
    return { value: t("tools.dashpayInfo.startup.starting") };
  },
  get ready() {
    return { value: t("tools.dashpayInfo.startup.ready") };
  },
  get noIdentity() {
    return { value: t("tools.dashpayInfo.startup.noIdentity") };
  },
  get partialNoIdentity() {
    return { value: t("tools.dashpayInfo.startup.partialNoIdentity"), note: t("tools.dashpayInfo.startup.looksAgainNextStart") };
  },
  get discoveryFailed() {
    return { value: t("tools.dashpayInfo.startup.discoveryFailed"), note: t("tools.dashpayInfo.startup.looksAgainNextStart") };
  },
  // The library's count is what is certain, not why (an unlock, the DIP-15 rescan, or no DashPay pass yet).
  get partialAccountsPending() {
    return { value: t("tools.dashpayInfo.startup.partialAccountsPending") };
  },
  get seedBindingUnverified() {
    return { value: t("tools.dashpayInfo.startup.seedBindingUnverified"), note: COPY.unverified.message };
  },
  get identityScanIncomplete() {
    return { value: t("tools.dashpayInfo.startup.identityScanIncomplete"), note: t("tools.dashpayInfo.startup.looksAgainNextStart") };
  },
  get identityUnsettled() {
    return { value: t("tools.dashpayInfo.startup.identityUnsettled"), note: COPY.unlockToLook.message };
  },
};

const LOOP_TITLE: Readonly<Record<SyncLoop, string>> = messageTable("tools.dashpayInfo.loops", [
  "identitySync",
  "dashPaySync",
  "dpnsSync",
  "platformAddressSync",
]);

/** D5: where Platform proofs get their quorum keys, and what that means for what the card shows. */
export function quorumRow(source: QuorumSource | undefined, trustedOnly: TrustedOnlyRead): InfoRow {
  const title = t("tools.dashpayInfo.quorumKeys");
  switch (source) {
    case undefined:
      return { title, value: NONE };
    case "spv":
      return { title, value: t("tools.dashpayInfo.quorum.spv") };
    case "trustedFallback":
      return { title, value: t("tools.dashpayInfo.quorum.trustedFallback"), note: t("tools.dashpayInfo.quorum.trustedFallbackNote") };
    case "trusted":
      if (trustedOnly === "failed") return { title, value: t("tools.dashpayInfo.quorum.trustedFailed"), note: t("tools.dashpayInfo.quorum.trustedFailedNote") };
      return trustedOnly === true
        ? { title, value: t("tools.dashpayInfo.quorum.trustedOnly"), note: t("tools.dashpayInfo.quorum.trustedOnlyNote") }
        : { title, value: t("tools.dashpayInfo.quorum.trustedDegraded"), note: t("tools.dashpayInfo.quorum.trustedDegradedNote") };
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
  if (!loop.running) return { title, value: loop.lastRunAt === null ? t("tools.dashpayInfo.loopState.stopped") : t("tools.dashpayInfo.loopState.stoppedLastRun", { time: clock(loop.lastRunAt) }) };
  if (loop.lastRunAt === null) return { title, value: t("tools.dashpayInfo.loopState.runningNoRun") };
  if (loop.nextRunAt === null) return { title, value: t("tools.dashpayInfo.loopState.runningLastRun", { time: clock(loop.lastRunAt) }) };
  return { title, value: t("tools.dashpayInfo.loopState.runningLastAndNext", { last: clock(loop.lastRunAt), next: clock(loop.nextRunAt) }) };
}

/** D4: the last pass's end and what it found; "—" until a pass reports (`last_pass` is `None` today). */
export function lastPassRow(pass: SyncPassReport | null, timeZone?: string): InfoRow {
  const title = t("tools.dashpayInfo.lastPass");
  if (!pass) return { title, value: NONE, note: t("tools.dashpayInfo.pass.noPassYet") };
  const value = dateText(pass.finishedAt, timeZone);
  if (pass.failureCode !== null) return { title, value, note: t("tools.dashpayInfo.pass.failed", { code: pass.failureCode }) };
  const found = [
    pass.newRequests ? t("tools.dashpayInfo.pass.newRequests", { count: pass.newRequests }) : "",
    pass.newContacts ? t("tools.dashpayInfo.pass.newContacts", { count: pass.newContacts }) : "",
    pass.newPayments ? t("tools.dashpayInfo.pass.newPayments", { count: pass.newPayments }) : "",
  ].filter(Boolean);
  return { title, value, note: found.length ? found.join(t("common.listSeparator")) : t("tools.dashpayInfo.pass.nothingNew") };
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
  const pendingTitle = t("tools.dashpayInfo.pendingContacts");
  const rows: InfoRow[] = [
    { title: t("tools.dashpayInfo.identityScan"), ...verdict },
    lastPassRow(s.lastPass, i.timeZone),
    s.pendingContactCrypto === 0
      ? { title: pendingTitle, value: t("tools.dashpayInfo.none") }
      : { title: pendingTitle, value: String(s.pendingContactCrypto), note: INFO_COPY.pendingContactsNote },
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
  if (s.lastPass?.failureCode)
    return at("failed", "danger", INFO_COPY.passFailed.title, {
      line: INFO_COPY.passFailed.message(s.lastPass.failureCode),
      // n7: without an identity a retry has nothing to sync.
      ...(identity ? {} : { syncNow: held(absent ? INFO_COPY.heldNotSetUp : INFO_COPY.heldNoIdentityYet) }),
    });
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
