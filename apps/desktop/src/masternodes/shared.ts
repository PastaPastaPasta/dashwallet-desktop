// Shared masternodes' view model (MG-10 §2, MG-10b S5): the session render table keyed by
// `next` (D2), the funding lines and grant caps from one value (§2.3.2), the consent diff, the
// setup wizard's checks (§2.2), the stage strip, the badge set and the refusals as sentences
// (§3.8). The engine decides every stage and every exit; nothing here infers one.

import { isAppError } from "../api/backend";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { ShareTermsView } from "../api/generated/ShareTermsView";
import type { SharedSession } from "../api/generated/SharedSession";
import type { SharedWarning } from "../api/generated/SharedWarning";
import type { UnreadableSession } from "../api/generated/UnreadableSession";
import { t } from "../i18n";

export const COIN = 100_000_000;
export const SHARE_TOTAL = 1_000 * COIN;
export const SHARE_MIN = 100 * COIN;
export const MAX_SHARES = 8;
export const DEFAULT_FEE = 100_000;
export const MAX_SHARED_FEE = 10_000_000;
/** Early-period presets, blocks (§2.2): none, 3 months, 6 months, 1 year, 2 years. */
export const PERIOD_PRESETS = [
  { blocks: 0, key: "none" },
  { blocks: 52_560, key: "m3" },
  { blocks: 105_120, key: "m6" },
  { blocks: 210_240, key: "y1" },
  { blocks: 420_480, key: "y2" },
] as const;

/** What this build can act on: start, join, lock and approve, and with S5b signing and sending
 * the registration where sends are on. Recovery comes with EP 4. A slice shows and badges nothing
 * it can't act on (D14). */
export interface SharedSlice {
  sign: boolean;
  send: boolean;
}
export const S5_SLICE: SharedSlice = { sign: true, send: true };
/** The slice before sends: what mainnet renders. */
export const NO_SENDS: SharedSlice = { sign: false, send: false };

/** MG's ruling: this version sends shared-masternode transactions everywhere but mainnet. Reads,
 * approvals, standby create and export, and the maintenance starts stay on. */
export const sharedSendsOn = (network: Pick<NetworkInfo, "kind">) => network.kind !== "mainnet";

/** Why `network` turns off setting up and joining (Start, Join, Lock, Approve, Change fee, Unlock),
 * or null where they're on. Reads, Discard and Abandon stay either way. */
export const setupOff = (network: Pick<NetworkInfo, "kind">) => (sharedSendsOn(network) ? null : t("masternodes.shared.mainnetOff"));

/** A setup step a session sheet shows: its primary Join, Lock or Approve, or the coordinator's Change
 * fee (a draft nobody has joined) and Unlock (while signing). */
export type SetupStep = "join" | "lock" | "approve" | "fee" | "unlock";

export function setupSteps(s: SharedSession, slice: SharedSlice): SetupStep[] {
  const p = renderSession(s, slice).primary;
  const steps: SetupStep[] = p === "join" || p === "lock" || p === "approve" ? [p] : [];
  if (s.role === "coordinator" && s.stage.kind === "draft" && s.rows.slice(1).every((r) => !r.contributed)) steps.push("fee");
  if (s.role === "coordinator" && s.stage.kind === "signing") steps.push("unlock");
  return steps;
}

/** The slice the screens render under on `network`. */
export const sliceFor = (network: Pick<NetworkInfo, "kind">): SharedSlice => (sharedSendsOn(network) ? S5_SLICE : NO_SENDS);

/** B7, before Create invitation and Join: its last sentence ("can't sign or send yet") only where
 * sends are off. */
export const reservationNote = (network: Pick<NetworkInfo, "kind">) =>
  sharedSendsOn(network) ? t("masternodes.shared.reservationNoteSends") : t("masternodes.shared.reservationNote");

/** A session sheet's actions. `copy` is the message to copy or save; the rest are calls. */
export type SharedAction = "join" | "copy" | "lock" | "approve" | "sign" | "send" | "resend";

/** One row of §2.3.1's registration table. */
export type RowKey =
  | "join"
  | "waitingReplies"
  | "sendReply"
  | "lock"
  | "approve"
  | "sendApproval"
  | "approvedCount"
  | "sign"
  | "sendSigned"
  | "waitingSigned"
  | "signLater"
  | "signedOut"
  | "abandoning"
  | "send"
  | "sent"
  | "confirmed"
  | "dead"
  | "conflicted"
  | "unknown";

export interface SessionRender {
  row: RowKey;
  /** The primary call, absent where this slice can't make it. */
  primary: Exclude<SharedAction, "copy"> | null;
  /** The message to copy or save now. */
  copy: boolean;
  /** Whether §2.3.1 lists this `(kind, role, stage, next)`; an unknown one renders its `next`
   * generically and fails the fixture test. */
  known: boolean;
}

const isRegistration = (s: SharedSession) => s.kind === "registration";
const coordinator = (s: SharedSession) => s.role === "coordinator";

/** §2.3.1, registration rows: the render key is `next` (D2), read with the stage and role. */
export function renderSession(s: SharedSession, slice: SharedSlice = NO_SENDS): SessionRender {
  const r = (row: RowKey, primary: SessionRender["primary"], copy = false, known = true): SessionRender => ({ row: row === "signLater" && !s.discardable ? "signedOut" : row, primary, copy, known });
  if (!isRegistration(s)) return r("unknown", null, false, false);
  const stage = s.stage.kind;
  // This wallet signed a spend back (D50): nothing hands the session's message or transaction on.
  if (s.abandonSpend && s.next === "nothing" && stage !== "confirmed" && stage !== "conflicted" && stage !== "dead") return r("abandoning", null);
  const c = coordinator(s);
  switch (s.next) {
    case "join":
      return stage === "invited" && !c ? r("join", "join") : r("unknown", "join", false, false);
    case "lock":
      return stage === "draft" && c ? r("lock", "lock") : r("unknown", "lock", false, false);
    case "approve":
      return stage === "signing" && !c ? r("approve", "approve") : r("unknown", "approve", false, false);
    case "sign":
      if (stage !== "combined" || c) return r("unknown", slice.sign ? "sign" : null, false, false);
      return slice.sign ? r("sign", "sign") : r("signLater", null);
    case "send":
      if (!slice.send) return (stage === "combined" || stage === "funding_signed") && c ? r("signLater", null) : r("unknown", null, false, false);
      return (stage === "combined" || stage === "funding_signed") && c ? r("send", "send") : r("unknown", "send", false, false);
    case "copy":
      if (stage === "draft") return r(c ? "waitingReplies" : "sendReply", null, true);
      if (stage === "signing") return r(c ? "approvedCount" : "sendApproval", null, true);
      if (stage === "funding_signed" && !c) return slice.sign ? r("sendSigned", null, true) : r("signLater", null);
      if ((stage === "combined" || stage === "funding_signed") && c) return slice.sign ? r("waitingSigned", null, true) : r("signLater", null);
      return r("unknown", null, true, false);
    case "wait":
      if (stage !== "sent") return r("unknown", null, false, false);
      return r("sent", c && slice.send ? "resend" : null);
    case "create_standby":
    case "nothing":
      if (stage === "confirmed") return r("confirmed", null);
      if (stage === "dead") return r("dead", null);
      if (stage === "conflicted") return r("conflicted", null);
      return r("unknown", null, false, false);
    default:
      return r("unknown", null, false, false);
  }
}

export type SendAction = "sign" | "send" | "resend";

/** The Sign, Send or Send again a session sheet offers on `network`: only where this network sends,
 * and only with the exact `debit` that caps its grant (no `debit`, no offer). */
export function sendOffer(s: SharedSession, network: Pick<NetworkInfo, "kind">, watchOnly: boolean): SendAction | null {
  const v = renderSession(s, sliceFor(network));
  const p = v.primary;
  if (!v.known || (p !== "sign" && p !== "send" && p !== "resend")) return null;
  return watchOnly || grantCap(p, s) === null ? null : p;
}

/** Whether mainnet holds back a Sign, Send or Send again this session would offer elsewhere. */
export const mainnetHoldsBack = (s: SharedSession, network: Pick<NetworkInfo, "kind">) =>
  !sharedSendsOn(network) && renderSession(s, S5_SLICE).primary !== renderSession(s, NO_SENDS).primary;

/** §3.6's S5 badge entries: registration sessions whose `next` this slice acts on. Never `copy`,
 * `wait` or `nothing`, and never a step whose screen the slice doesn't build. */
export function sessionNeedsAction(s: SharedSession, slice: SharedSlice = NO_SENDS): boolean {
  if (!isRegistration(s)) return false;
  const v = renderSession(s, slice);
  return v.known && v.primary !== null && v.primary !== "resend";
}

export const sharedBadgeCount = (sessions: readonly SharedSession[] | undefined, slice: SharedSlice = NO_SENDS) =>
  (sessions ?? []).filter((s) => sessionNeedsAction(s, slice)).length;

/** The unreadable rows "Sessions in progress" lists: one it may discard, or one still holding
 * reserved coins. Once the follower released its locks nothing of it is left to show. */
export const unreadableInProgress = (rows: readonly UnreadableSession[] | undefined): UnreadableSession[] => (rows ?? []).filter((s) => s.discardable || s.reserved.length > 0);

/** "Sessions in progress" (§2.1): not settled, newest first. Confirmed and conflicted sessions
 * leave the list; a dead one stays until it is discarded. */
export function sessionsInProgress(sessions: readonly SharedSession[] | undefined): SharedSession[] {
  return (sessions ?? []).filter((s) => isRegistration(s) && s.stage.kind !== "confirmed" && s.stage.kind !== "conflicted").sort((a, b) => b.updatedAt - a.updatedAt || a.id.localeCompare(b.id));
}

/** The stage strip's current step, 1–6 (MG-00 D31 over MG-04b §2.3). */
export function stageStep(s: SharedSession): number {
  switch (s.stage.kind) {
    case "invited":
      return 1;
    case "draft":
      return s.next === "lock" ? 3 : 2;
    case "signing":
      return 4;
    case "combined":
    case "funding_signed":
      return 5;
    default:
      return 6;
  }
}

export interface DebitParts {
  debit: number;
  share: number;
  /** The coordinator's fee; 0 for a participant. */
  fee: number;
  /** Change too small to keep, which goes to the network fee. */
  dust: number;
}

/** §2.3.2 after selection: the exact `debit` and its parts; `null` before coins are picked. */
export function debitParts(s: SharedSession): DebitParts | null {
  if (s.debit === null) return null;
  const mine = s.rows.find((r) => r.mine);
  if (!mine) return null;
  const fee = coordinator(s) ? (s.fee ?? 0) : 0;
  return { debit: s.debit, share: mine.amount, fee, dust: Math.max(0, s.debit - mine.amount - fee) };
}

/** The grant cap for a session call (§3.4): 0 for lock and approve (they sign consent only), and
 * exactly the session's `debit` for sign, send and resend, the amount their confirm prints (D18,
 * dwcli's `debit_grant`). `null` without a `debit`: the sheet then offers no such call. */
export function grantCap(action: Exclude<SharedAction, "copy" | "join">, s: SharedSession): number | null {
  if (action === "lock" || action === "approve") return 0;
  return s.debit;
}

/** The stored hand-off codes the engine reads as refused (its `outcome_of_code`, MG-10 §4.4 rule
 * 3): nothing left. Any other code, the resend marker included, may have left. */
export const REFUSED_HANDOFF: readonly string[] = ["send.no_peers", "send.broadcast_rejected", "send.prepared_tx_spent", "send.cancelled", "network_not_open", "wallet_not_found", "send.held_until_restart"];

/** §2.3.3's hand-off line under a sent registration, `null` when the hand-off left no error. A
 * refused code reads as `refused`'s sentence; an unknown one as this sheet's own lower-case
 * sentence (the resend marker, the timeout), else as the bare code. */
export function handoffLine(s: SharedSession, refused: (code: string) => string | null): string | null {
  if (s.stage.kind !== "sent" || !s.lastError) return null;
  const code = s.lastError;
  const bare = () => t("masternodes.shared.row.handoffCode", { code });
  if (REFUSED_HANDOFF.includes(code)) {
    return t("masternodes.shared.row.handoffRefused", { sentence: (refused(code) ?? bare()).replace(/[.。]\s*$/, "") });
  }
  const sentence =
    code === "send.resend_in_progress" ? t("masternodes.shared.row.handoffResending") : code === "send.broadcast_unknown" ? t("masternodes.shared.row.handoffTimeout") : bare();
  return t("masternodes.shared.row.handoffUnknown", { sentence });
}

/** D50's line for the spend back while it is pending, by its state; `null` without one. "Didn't go
 * out" only for a recorded refusal, whose `{sentence}` is the refusal's own. */
export function spendBackLine(s: SharedSession, refused: (code: string) => string | null): string | null {
  const a = s.abandonSpend;
  if (!a?.state) return null;
  const txid = a.txid.slice(0, 8);
  switch (a.state) {
    case "uncertain":
      return t("masternodes.shared.spendBack.uncertain", { txid });
    case "refused": {
      const code = a.code ?? "";
      const sentence = (refused(code) ?? t("masternodes.shared.row.handoffCode", { code })).replace(/[.。]\s*$/, "");
      return t("masternodes.shared.spendBack.refused", { sentence });
    }
    case "in_history":
      return t("masternodes.shared.spendBack.inHistory", { txid });
    case "abandoned_in_activity":
      return t("masternodes.shared.spendBack.cancelled", { txid });
    case "legacy":
      return t("masternodes.shared.spendBack.legacy", { txid });
  }
}

/** The spend back's own action (D50): Send again replays the same bytes while the read lists it;
 * Try again (refused) and Abandon again (cancelled in Activity) are a new spend of the same coins. */
export function spendBackAction(s: SharedSession): "resend" | "tryAgain" | "abandonAgain" | null {
  const state = s.abandonSpend?.state;
  if (state === "refused") return canAbandon(s) ? "tryAgain" : null;
  if (state === "abandoned_in_activity") return canAbandon(s) ? "abandonAgain" : null;
  return s.resendAbandonCharge !== null && (state === "uncertain" || state === "in_history") ? "resend" : null;
}

/** `row.signedOut` with its `{abandon}` filled by the Abandon button's own label. Shown only
 * where `canAbandon` holds: the line tells the user to choose a button that must be there. */
export const signedOutText = () => t("masternodes.shared.row.signedOut", { abandon: t("masternodes.shared.session.abandon") });

/** Abandon's refusal for want of a coin to spend (`shared_stage` naming the out stage): the
 * wallet's own transaction spends the reserved coins first and is abandoned in Activity. */
export function abandonNeedsOwnFirst(e: unknown): boolean {
  if (!isAppError(e) || e.code !== "masternodes.shared_stage") return false;
  const stage = (e.data as SharedErrorData | null)?.stage;
  return stage === "sent" || stage === "funding_signed";
}

/** Whether the sheet offers Abandon (§9.3): a registration that may be out (`discardable`
 * false), not settled, with a reserved coin, whose recovery read lists Abandon (`abandonCharge`,
 * from the same coin test the call spends by). */
export function canAbandon(s: SharedSession): boolean {
  const settled = s.stage.kind === "confirmed" || s.stage.kind === "conflicted" || s.stage.kind === "dead";
  return isRegistration(s) && !s.discardable && !settled && s.reserved.length > 0 && s.abandonable && s.abandonCharge !== null;
}

/** MG's way-out line: wherever this wallet holds the registration (`txid`), at `sent`, and
 * wherever `row.signedOut` or `unvalued_inputs_out` is hidden because Abandon isn't offered.
 * Collapsed until the session is `overdue` (D49), then expanded; `null` where it isn't shown. */
export function wayOutShown(s: SharedSession, row: RowKey): "collapsed" | "expanded" | null {
  const settled = s.stage.kind === "confirmed" || s.stage.kind === "conflicted" || s.stage.kind === "dead";
  if (!s.txid || !isRegistration(s) || settled) return null;
  const shown = s.stage.kind === "sent" || (!canAbandon(s) && (row === "signedOut" || (!s.discardable && s.warnings.some((w) => w.kind === "unvalued_inputs"))));
  if (!shown) return null;
  return s.overdue ? "expanded" : "collapsed";
}

/** The session sheet's Network fee row value, under its own "Network fee" label: the fee for the
 * coordinator who pays it, who pays it for a participant; `null` without a fee. */
export function feeRowValue(s: SharedSession, dash: (duffs: number) => string, coordinatorLabel: string): string | null {
  if (s.fee === null) return null;
  return coordinator(s) ? t("masternodes.shared.session.feeYoursValue", { fee: dash(s.fee) }) : t("masternodes.shared.session.feePayerValue", { coordinator: coordinatorLabel });
}

/** The exact funding line (§2.3.2): what leaves this wallet and why, from `debit`. */
export function fundingLine(s: SharedSession, dash: (duffs: number) => string): string | null {
  const p = debitParts(s);
  if (!p) return null;
  const v = { debit: dash(p.debit), share: dash(p.share), fee: dash(p.fee), dust: dash(p.dust) };
  if (coordinator(s)) return t(p.dust > 0 ? "masternodes.shared.funding.coordinatorDust" : "masternodes.shared.funding.coordinator", v);
  return t(p.dust > 0 ? "masternodes.shared.funding.participantDust" : "masternodes.shared.funding.participant", v);
}

/** The estimate before coins are picked (§2.3.2): never the basis of a grant. */
export function joinEstimate(s: SharedSession, shareIndex: number | null, dash: (duffs: number) => string): string | null {
  const row = shareIndex === null ? s.rows.find((r) => !r.ownerAddress) : s.rows.find((r) => r.index === shareIndex);
  return row ? t("masternodes.shared.funding.estimate", { amount: dash(row.amount) }) : null;
}

export interface TermChange {
  field: "service" | "operator" | "operatorReward" | "voting" | "earlyPeriodBlocks" | "earlyPenalty";
  before: string;
  after: string;
}

/** The consent diff (§2.3.1 approve): each term changed since this wallet last approved. */
export function termsDiff(approved: ShareTermsView | null, now: ShareTermsView, dash: (duffs: number) => string): TermChange[] {
  if (!approved) return [];
  const out: TermChange[] = [];
  const push = (field: TermChange["field"], a: string, b: string) => {
    if (a !== b) out.push({ field, before: a, after: b });
  };
  push("service", approved.service, now.service);
  push("operator", approved.operatorPublicKey, now.operatorPublicKey);
  push("operatorReward", basisPoints(approved.operatorReward), basisPoints(now.operatorReward));
  push("voting", approved.votingAddress, now.votingAddress);
  push("earlyPeriodBlocks", String(approved.earlyPeriodBlocks), String(now.earlyPeriodBlocks));
  push("earlyPenalty", dash(approved.earlyPenalty), dash(now.earlyPenalty));
  return out;
}

/** Basis points as a percent, "5" or "2.5". */
export const basisPoints = (bp: number) => String(bp / 100);

export interface SetupShare {
  label: string;
  /** Duffs. */
  amount: number;
}

export type SetupError = "count" | "min" | "sum" | "label" | "duplicate";

/** §2.2's Shares step, mirroring MG-04b §5 before any call. */
export function shareErrors(shares: readonly SetupShare[]): SetupError[] {
  const errors: SetupError[] = [];
  if (shares.length < 2 || shares.length > MAX_SHARES) errors.push("count");
  if (shares.some((s) => s.amount < SHARE_MIN)) errors.push("min");
  if (shares.reduce((a, s) => a + s.amount, 0) !== SHARE_TOTAL) errors.push("sum");
  const labels = shares.map((s) => s.label.trim());
  if (labels.some((l) => l === "")) errors.push("label");
  else if (new Set(labels).size !== labels.length) errors.push("duplicate");
  return errors;
}

/** §2.2's Exit terms step: the penalty is below the smallest share, and 0 without a period. */
export function penaltyError(shares: readonly SetupShare[], periodBlocks: number, penalty: number): "penalty" | "penaltyNoPeriod" | null {
  if (periodBlocks === 0) return penalty === 0 ? null : "penaltyNoPeriod";
  const smallest = Math.min(...shares.map((s) => s.amount));
  return penalty < smallest ? null : "penalty";
}

/** A whole-Dash entry ("400", "12.5", "0,001") as duffs; `null` when it isn't one. The setup's
 * share, penalty and fee fields are in Dash whatever the display unit (§4.25.8). */
export function parseDash(text: string): number | null {
  const m = /^(\d{1,5})(?:[.,](\d{1,8}))?$/.exec(text.trim());
  if (!m) return null;
  return Number(m[1]) * COIN + Number((m[2] ?? "").padEnd(8, "0"));
}

/** Duffs as the plain Dash text the setup's fields take back. */
export function dashText(duffs: number): string {
  const whole = Math.floor(duffs / COIN);
  const frac = String(duffs % COIN).padStart(8, "0").replace(/0+$/, "");
  return frac ? `${whole}.${frac}` : String(whole);
}

/** An operator public key as typed: 96 hex is a key; 64 hex is a pasted secret (D27). */
export function operatorKeyProblem(text: string): "secret" | "invalid" | null {
  const k = text.trim();
  if (/^[0-9a-fA-F]{96}$/.test(k)) return null;
  return /^[0-9a-fA-F]{64}$/.test(k) ? "secret" : "invalid";
}

/** The operator's share as typed ("5", "2.5", "12.25") in basis points; `null` past 100 % or two decimals. */
export function parsePercent(text: string): number | null {
  const m = /^(\d{1,3})(?:[.,](\d{1,2}))?$/.exec(text.trim());
  if (!m) return null;
  const bp = Number(m[1]) * 100 + Number((m[2] ?? "").padEnd(2, "0"));
  return bp <= 10_000 ? bp : null;
}

/** An IPv4 `a.b.c.d:port` (G3): the wallet's own forms take nothing else. */
export function isIpv4Service(text: string): boolean {
  const m = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3}):(\d{1,5})$/.exec(text.trim());
  if (!m) return false;
  const octets = m.slice(1, 5).map(Number);
  const port = Number(m[5]);
  return octets.every((o, i) => o <= 255 && String(o) === m[i + 1]) && port >= 1 && port <= 65_535;
}

/** Labels for a warning's share numbers. */
export function shareLabels(s: SharedSession | null, shares: readonly number[]): string {
  return shares.map((i) => s?.rows.find((r) => r.index === i)?.label ?? t("masternodes.shared.session.share") + ` ${i + 1}`).join(", ");
}

/** A session or import warning as a sentence (§3.8); `null` for one this sheet doesn't show. */
export function warningText(w: SharedWarning, s: SharedSession | null): string | null {
  switch (w.kind) {
    case "edited_message":
      return t("masternodes.shared.warning.edited_message");
    case "unvalued_inputs":
      // Out, the line tells the user to abandon: only where Abandon is offered.
      if (s?.discardable === false) return canAbandon(s) ? t("masternodes.shared.warning.unvalued_inputs_out", { labels: shareLabels(s, w.shares) }) : null;
      return t("masternodes.shared.warning.unvalued_inputs", { labels: shareLabels(s, w.shares) });
    case "empty_service":
      return t("masternodes.shared.warning.empty_service");
    case "zero_penalty":
      return t("masternodes.shared.warning.zero_penalty");
    case "skipped_signatures":
      return t("masternodes.shared.warning.skipped_signatures", { count: w.count, labels: shareLabels(s, w.shares) });
    case "no_approvals":
      return t("masternodes.shared.warning.no_approvals");
    default:
      return null;
  }
}

interface SharedErrorData {
  kind?: string;
  reason?: string;
  share?: number | null;
  stage?: string;
  needed?: number | null;
}

/** A shared call's refusal as one sentence (§3.8), never echoing pasted text. */
export function sharedErrorText(e: unknown, ctx: { network: string; session?: SharedSession | null; dash: (duffs: number) => string }): string {
  if (!isAppError(e)) return t("masternodes.shared.error.fallback", { code: String(e) });
  const data = (e.data ?? {}) as SharedErrorData;
  switch (e.code) {
    case "masternodes.shared_message": {
      const reason = data.reason ?? "";
      if (reason === "network") return t("masternodes.shared.error.network", { network: ctx.network });
      if (reason === "bad_signature") {
        const label = data.share != null ? ctx.session?.rows.find((r) => r.index === data.share)?.label : undefined;
        return label ? t("masternodes.shared.error.bad_signature", { label }) : t("masternodes.shared.error.bad_signature_unknown");
      }
      const known = ["not_shared", "too_large", "parse", "other_session", "terms_changed", "older", "conflict", "not_participant", "labels"] as const;
      const k = known.find((x) => x === reason);
      return k ? t(`masternodes.shared.error.${k}`) : t("masternodes.shared.error.fallback", { code: `${e.code}{${reason}}` });
    }
    case "masternodes.shared_funding_mismatch":
      return data.reason === "fee_low" && data.needed != null ? t("masternodes.shared.error.fee_low", { needed: ctx.dash(data.needed) }) : t("masternodes.shared.error.funding");
    case "masternodes.shared_stage":
      if (data.stage === "replied") return t("masternodes.shared.error.replied");
      if (data.stage === "unsigned") return t("masternodes.shared.error.unsigned");
      if (data.stage === "abandoned") return t("masternodes.shared.error.abandoned");
      return t("masternodes.shared.error.stage");
    case "masternodes.unsupported":
      return t("masternodes.shared.error.unsupported");
    case "masternodes.shared_session_not_found":
      return t("masternodes.shared.error.notFound");
    case "send.grant_exceeded":
      return t("masternodes.shared.error.grantExceeded");
    case "app.file_read":
      return t("masternodes.shared.error.fileRead");
    case "app.file_too_large":
      return t("masternodes.shared.error.too_large");
    case "app.shared_send_off":
      return t("masternodes.shared.error.mainnet_send_off");
    case "app.shared_send_unsupported":
      return t("masternodes.shared.error.send_unsupported");
    default:
      return t("masternodes.shared.error.fallback", { code: e.code });
  }
}
