// Shared masternodes' view model (MG-10 §2, MG-10b S5, S6): the session render table keyed by
// `next` (D2), the funding lines and grant caps from one value (§2.3.2), the consent diff, the
// setup wizard's checks (§2.2), the stage strip, the badge set, the refusals as sentences (§3.8),
// and a confirmed shared masternode's pane lines (§2.4, §2.8). The engine decides every stage,
// every exit and every validity; nothing here infers one.

import { isAppError } from "../api/backend";
import type { DissolvePreview } from "../api/generated/DissolvePreview";
import type { MasternodeRow } from "../api/generated/MasternodeRow";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { ShareFact } from "../api/generated/ShareFact";
import type { ShareTermsView } from "../api/generated/ShareTermsView";
import type { SharedEligibilityName } from "../api/generated/SharedEligibilityName";
import type { SharedSession } from "../api/generated/SharedSession";
import type { SharedShares } from "../api/generated/SharedShares";
import type { SharedWarning } from "../api/generated/SharedWarning";
import type { Standby } from "../api/generated/Standby";
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

/** What this build can act on. S5 in 0.4 renders start, join, lock and approve; signing, sending
 * and recovery come with EP 4 (S5b). A slice shows and badges nothing it can't act on (D14). */
export interface SharedSlice {
  /** A registration's funding signature. */
  sign: boolean;
  /** A registration's send. */
  send: boolean;
  /** Dissolve-together and key-change sessions render, with their approvals. */
  maintenance?: boolean;
  /** A complete dissolution together is sent from here (never a key change in S6). */
  maintenanceSend?: boolean;
}
export const S5_SLICE: SharedSlice = { sign: false, send: false };
/** S6: maintenance sessions and their approvals; a dissolution together sends where the network
 * allows it; a key change never sends; registrations keep S5's rule. */
export const S6_SLICE: SharedSlice = { sign: false, send: false, maintenance: true, maintenanceSend: true };

/** MG's ruling: this version sends shared-masternode transactions everywhere but mainnet. Reads,
 * approvals, standby create and export, and the maintenance starts stay on. */
export const sharedSendsOn = (network: Pick<NetworkInfo, "kind">) => network.kind !== "mainnet";

/** The slice the screens render under on `network`. */
export const sliceFor = (network: Pick<NetworkInfo, "kind">): SharedSlice => ({ ...S6_SLICE, maintenanceSend: sharedSendsOn(network) });

/** A session sheet's actions. `copy` is the message to copy or save; the rest are calls. */
export type SharedAction = "join" | "copy" | "lock" | "approve" | "sign" | "send" | "approveMaintenance" | "sendMaintenance";

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
  | "send"
  | "sent"
  | "confirmed"
  | "dead"
  | "conflicted"
  | "togetherApprove"
  | "togetherSendApproval"
  | "togetherSend"
  | "togetherSendOff"
  | "dissolved"
  | "keyApprove"
  | "keyWaiting"
  | "keySendApproval"
  | "keySendSignatures"
  | "keySendLater"
  | "keyStale"
  | "keyStaleOther"
  | "keyConfirmed"
  | "keyDead"
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

const row = (row: RowKey, primary: SessionRender["primary"], copy = false, known = true): SessionRender => ({ row, primary, copy, known });

/** §2.3.1, registration rows: the render key is `next` (D2), read with the stage and role. */
export function renderSession(s: SharedSession, slice: SharedSlice = S5_SLICE): SessionRender {
  if (s.kind === "dissolve_together" && slice.maintenance) return renderTogether(s, slice);
  if (s.kind === "key_change" && slice.maintenance) return renderKeyChange(s);
  if (!isRegistration(s)) return row("unknown", null, false, false);
  const stage = s.stage.kind;
  const c = coordinator(s);
  switch (s.next) {
    case "join":
      return stage === "invited" && !c ? row("join", "join") : row("unknown", "join", false, false);
    case "lock":
      return stage === "draft" && c ? row("lock", "lock") : row("unknown", "lock", false, false);
    case "approve":
      return stage === "signing" && !c ? row("approve", "approve") : row("unknown", "approve", false, false);
    case "sign":
      return stage === "combined" && !c ? row("sign", slice.sign ? "sign" : null) : row("unknown", slice.sign ? "sign" : null, false, false);
    case "send":
      if (!slice.send) return (stage === "combined" || stage === "funding_signed") && c ? row("signLater", null) : row("unknown", null, false, false);
      return (stage === "combined" || stage === "funding_signed") && c ? row("send", "send") : row("unknown", "send", false, false);
    case "copy":
      if (stage === "draft") return row(c ? "waitingReplies" : "sendReply", null, true);
      if (stage === "signing") return row(c ? "approvedCount" : "sendApproval", null, true);
      if (stage === "funding_signed" && !c) return slice.sign ? row("sendSigned", null, true) : row("signLater", null);
      if ((stage === "combined" || stage === "funding_signed") && c) return slice.sign ? row("waitingSigned", null, true) : row("signLater", null);
      return row("unknown", null, true, false);
    case "wait":
      return stage === "sent" ? row("sent", null) : row("unknown", null, false, false);
    case "create_standby":
    case "nothing":
      if (stage === "confirmed") return row("confirmed", null);
      if (stage === "dead") return row("dead", null);
      if (stage === "conflicted") return row("conflicted", null);
      return row("unknown", null, false, false);
    default:
      return row("unknown", null, false, false);
  }
}

/** §2.3.1, dissolve-together rows. The send is the one maintenance send S6 makes, and only where the
 * network allows it. */
function renderTogether(s: SharedSession, slice: SharedSlice): SessionRender {
  const stage = s.stage.kind;
  switch (s.next) {
    case "approve":
      return stage === "signing" ? row("togetherApprove", "approveMaintenance") : row("unknown", "approveMaintenance", false, false);
    case "copy":
      return stage === "signing" || stage === "combined" ? row("togetherSendApproval", null, true) : row("unknown", null, true, false);
    case "send":
      if (stage !== "combined") return row("unknown", null, false, false);
      return slice.maintenanceSend ? row("togetherSend", "sendMaintenance") : row("togetherSendOff", null);
    case "wait":
      return stage === "sent" ? row("sent", null) : row("unknown", null, false, false);
    case "nothing":
    case "create_standby":
      if (stage === "confirmed") return row("dissolved", null);
      if (stage === "conflicted" || stage === "dead") return row("conflicted", null);
      return row("unknown", null, false, false);
    default:
      return row("unknown", null, false, false);
  }
}

/** Whether this wallet funds a key change: it alone may send it (D99). */
export const isFunder = (s: SharedSession) => s.keyChange?.funder ?? s.role === "coordinator";

/** §2.3.1 and §2.6, key-change rows. S6 never sends a key change: the funder's complete change
 * reads "later", with Discard while the engine says it is discardable. */
function renderKeyChange(s: SharedSession): SessionRender {
  const stage = s.stage.kind;
  const funder = isFunder(s);
  switch (s.next) {
    case "approve":
      return stage === "signing" ? row("keyApprove", "approveMaintenance") : row("unknown", "approveMaintenance", false, false);
    case "copy":
      if (stage === "signing") return row(funder ? "keyWaiting" : "keySendApproval", null, true);
      if (stage === "combined" && !funder) return row("keySendSignatures", null, true);
      return row("unknown", null, true, false);
    case "send":
      return stage === "combined" && funder ? row("keySendLater", null) : row("unknown", null, false, false);
    case "wait":
      return stage === "sent" ? row("sent", null) : row("unknown", null, false, false);
    case "abandon":
      return stage === "stale" && funder ? row("keyStale", null) : row("unknown", null, false, false);
    case "nothing":
    case "create_standby":
      if (stage === "stale") return row(funder ? "keyStale" : "keyStaleOther", null);
      if (stage === "confirmed") return row("keyConfirmed", null);
      if (stage === "dead") return row("keyDead", null);
      if (stage === "conflicted") return row("conflicted", null);
      return row("unknown", null, false, false);
    default:
      return row("unknown", null, false, false);
  }
}

/** §3.6's badge entries: sessions whose `next` this slice acts on. Never `copy`, `wait` or
 * `nothing`, and never a step whose screen the slice doesn't build. */
export function sessionNeedsAction(s: SharedSession, slice: SharedSlice = S5_SLICE): boolean {
  const v = renderSession(s, slice);
  return v.known && v.primary !== null;
}

export const sharedBadgeCount = (sessions: readonly SharedSession[] | undefined, slice: SharedSlice = S5_SLICE) =>
  (sessions ?? []).filter((s) => sessionNeedsAction(s, slice)).length;

/** The final stages (F4): never offered Discard, and out of "Sessions in progress". The engine reads
 * a dead session as discardable until something of it was sent; the UI doesn't offer it. */
export const isFinal = (s: Pick<SharedSession, "stage">) => s.stage.kind === "confirmed" || s.stage.kind === "conflicted" || s.stage.kind === "dead";

/** Discard is offered only while the engine says so, and never at a final stage. */
export const canDiscard = (s: Pick<SharedSession, "discardable">) => s.discardable;

/** "Sessions in progress" (§2.1): not final, or final but still discardable (a dead session whose
 * coins Discard releases), newest first. Registrations, and from S6 the maintenance sessions too. */
export function sessionsInProgress(sessions: readonly SharedSession[] | undefined, slice: SharedSlice = S5_SLICE): SharedSession[] {
  return (sessions ?? []).filter((s) => (isRegistration(s) || slice.maintenance) && (!isFinal(s) || s.discardable)).sort((a, b) => b.updatedAt - a.updatedAt || a.id.localeCompare(b.id));
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

/** The grant cap for a session call (§3.4): 0 for lock and approve (they sign consent only), the
 * same `debit` the confirm prints for sign and send. `null` when there is nothing to cap. */
export function grantCap(action: Exclude<SharedAction, "copy" | "join">, s: SharedSession): number | null {
  if (action === "lock" || action === "approve") return 0;
  return debitParts(s)?.debit ?? null;
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
      return t("masternodes.shared.warning.unvalued_inputs", { labels: shareLabels(s, w.shares) });
    case "empty_service":
      return t("masternodes.shared.warning.empty_service");
    case "zero_penalty":
      return t("masternodes.shared.warning.zero_penalty");
    case "skipped_signatures":
      return t("masternodes.shared.warning.skipped_signatures", { count: w.count, labels: shareLabels(s, w.shares) });
    case "no_approvals":
      return t("masternodes.shared.warning.no_approvals");
    case "operator_reset":
      return `${t("masternodes.myMasternodes.changeKeys.poseWarning")} ${t("masternodes.myMasternodes.changeKeys.poseExplain")}`;
    case "usable_by_funder":
      return t("masternodes.shared.warning.usable_by_funder");
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
  fee?: number;
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
      const known = ["not_shared", "too_large", "parse", "other_session", "terms_changed", "older", "conflict", "not_participant", "labels", "stale", "not_key_change", "inputs_hash", "other_masternode"] as const;
      const k = known.find((x) => x === reason);
      return k ? t(`masternodes.shared.error.${k}`) : t("masternodes.shared.error.fallback", { code: `${e.code}{${reason}}` });
    }
    case "masternodes.shared_funding_mismatch":
      return data.reason === "fee_low" && data.needed != null ? t("masternodes.shared.error.fee_low", { needed: ctx.dash(data.needed) }) : t("masternodes.shared.error.funding");
    case "masternodes.shared_stage":
      return data.stage === "replied" ? t("masternodes.shared.error.replied") : t("masternodes.shared.error.stage");
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
    case "masternodes.shared_actor_fee":
      return t("masternodes.shared.error.actor_fee", { fee: ctx.dash(data.fee ?? 0) });
    case "masternodes.update_pending":
      return t("masternodes.myMasternodes.errors.update_pending");
    case "masternodes.in_flight":
      return t("masternodes.myMasternodes.errors.in_flight");
    case "masternodes.share_not_mine":
      return t("masternodes.myMasternodes.errors.share_not_mine");
    case "masternodes.shared_too_early":
      return t("masternodes.myMasternodes.errors.shared_too_early");
    case "masternodes.shared_registration_unknown":
      return t("masternodes.myMasternodes.errors.shared_registration_unknown");
    case "vault.locked":
      return t("masternodes.myMasternodes.errors.vault_locked");
    default:
      return t("masternodes.shared.error.fallback", { code: e.code });
  }
}

// ---------------------------------------------------------------- a shared masternode (§2.4, S6)

/** A row the shared pane reads: it holds shares, or the engine marks it shared and it is the wallet's. */
export const isSharedRow = (r: Pick<MasternodeRow, "shares" | "sharedUnsupported" | "mine">) => r.shares !== null || (r.sharedUnsupported && r.mine);

/** "you hold 1 of 3 shares": the row's count, else EP 5's. */
export function heldShares(r: Pick<MasternodeRow, "shares">, shares: SharedShares | undefined): { held: number; of: number } | null {
  if (r.shares) return r.shares;
  if (!shares) return null;
  return { held: shares.shares.filter((s) => s.mine).length, of: shares.shares.length };
}

/** The registration session of `hash` (§2.4): kind registration, confirmed, same ProTx hash. */
export function registrationSession(sessions: readonly SharedSession[] | undefined, hash: string, id: string | null = null): SharedSession | null {
  const all = (sessions ?? []).filter((s) => s.kind === "registration" && s.stage.kind === "confirmed" && s.proTxHash === hash);
  return all.find((s) => s.id === id) ?? all[0] ?? null;
}

export interface LegendEntry {
  index: number;
  /** The session's label, else "Share 1". */
  label: string;
  amount: number;
  mine: boolean;
  /** Percent of the collateral, for the bar. */
  percent: number;
}

/** The share bar and its legend from EP 5's shares, labelled from the session's rows when present. */
/** A share's label in the legend, "" when it has none. */
export const labelOf = (legend: readonly LegendEntry[], index: number) => legend.find((l) => l.index === index)?.label ?? "";

export function shareLegend(shares: SharedShares, session: SharedSession | null): LegendEntry[] {
  const total = shares.shares.reduce((a, s) => a + s.amount, 0);
  return shares.shares.map((s) => ({
    index: s.index,
    label: session?.rows.find((r) => r.index === s.index)?.label ?? t("masternodes.shared.detail.shareN", { n: s.index + 1 }),
    amount: s.amount,
    mine: s.mine,
    percent: total > 0 ? (s.amount * 100) / total : 0,
  }));
}

/** A percent with at most two decimals, "40" or "33.33". */
export const percentText = (p: number) => String(Math.round(p * 100) / 100);

/** Where this wallet believes a share's payouts go: only a confirmed update moves it (§2.4.1). */
export const payoutAddress = (f: ShareFact) => f.reward.latestConfirmedLocal?.address ?? f.reward.atRegistration;

/** The line for this wallet's latest unconfirmed reward change, or `null`. */
export function attemptLine(f: ShareFact, address: (a: string) => string, date: (secs: number) => string): string | null {
  const a = f.reward.latestAttempt;
  if (!a) return null;
  const v = { address: address(a.address) };
  switch (a.state) {
    case "signed":
    case "not_sent":
      return t("masternodes.shared.detail.attempt.notSent", v);
    case "sent":
      return a.sentAt === null ? t("masternodes.shared.detail.attempt.sentUndated", v) : t("masternodes.shared.detail.attempt.sent", { ...v, date: date(a.sentAt) });
    case "conflicted":
      return t("masternodes.shared.detail.attempt.conflicted", v);
  }
}

/** The block the early period ends at, or `null` without one. */
export const earlyEnd = (s: SharedShares) => (s.earlyPeriodBlocks > 0 ? s.registeredHeight + s.earlyPeriodBlocks : null);

/** §2.4.2's eligibility line in place of the maintenance actions; `null` for `ok`. */
export function eligibilityLine(e: SharedEligibilityName): string | null {
  return e === "ok" ? null : t(`masternodes.shared.detail.eligibility.${e}`);
}

export interface StandbyLine {
  variant: "full" | "immediate";
  text: string;
}

/** A stored standby's variants (§2.8 Shown), each validity from EP 5's `full_valid_now`, never a height comparison.
 * Once a held full refund is valid the engine refuses the immediate one, so it isn't listed. */
export function standbyLines(s: Standby, fullValidNow: boolean, dash: (duffs: number) => string): StandbyLine[] {
  const out: StandbyLine[] = [];
  if (s.full) out.push({ variant: "full", text: fullValidNow ? t("masternodes.shared.detail.standby.fullNow") : t("masternodes.shared.detail.standby.full", { height: s.fullValidFrom }) });
  if (s.immediate && !(s.full && fullValidNow))
    out.push({ variant: "immediate", text: s.penalty === null ? t("masternodes.shared.detail.standby.immediateUnknown") : t("masternodes.shared.detail.standby.immediate", { penalty: dash(s.penalty) }) });
  return out;
}

/** "Saved on this computer on Oct 15", or "Imported". */
export const standbySource = (s: Standby, date: (secs: number) => string) =>
  s.source === "imported" ? t("masternodes.shared.detail.standby.imported") : t("masternodes.shared.detail.standby.saved", { date: date(s.createdAt) });

// ---------------------------------------------------------------- maintenance (§2.6–§2.8, S6)

/** The fee a dissolution alone and a standby pay (§2.7, Q7), duffs. */
export const DISSOLVE_FEE = 1_000;
export const STANDBY_FEE = 1_000;

/** §2.7's together-fee default for `n` shares, duffs: the relay minimum of the combined size. */
export const togetherFee = (n: number) => Math.max(1_000, 300 + 110 * n);

/** Create standby's cap (§3.4): the penalty and the fee for each of this wallet's shares. */
export function standbyCreateCap(shares: SharedShares, fee: number = STANDBY_FEE): number {
  return shares.shares.filter((f) => f.mine).length * (shares.earlyPenalty + fee);
}

/** What a dissolution alone takes from the share, as the preview prints it and the grant caps it. */
export const dissolveCap = (p: DissolvePreview) => p.totalDebit;

/** The approve-maintenance cap (§3.4): a dissolution's actor fee once accepted, else 0. */
export function approveMaintenanceCap(s: SharedSession, acceptActorFee: boolean): number {
  return s.kind === "dissolve_together" && acceptActorFee ? (s.actorFee ?? 0) : 0;
}

/** What this wallet gets back from a dissolution together: its shares, less the fee its actor share pays. */
export function togetherReturn(s: SharedSession): number {
  const mine = s.rows.filter((r) => r.mine).reduce((a, r) => a + r.amount, 0);
  const fee = s.role === "coordinator" ? (s.fee ?? 0) : (s.actorFee ?? 0);
  return Math.max(0, mine - fee);
}

export interface StandbySend {
  variant: "full" | "immediate";
  /** The button's catalog key under `masternodes.shared.detail.standby`. */
  label: "sendFull" | "sendImmediate" | "sendIt";
  /** The grant's cap (§3.4): fee + penalty (immediate, own share), fee (full, own share), 0 (another's). */
  cap: number;
  /** What the actor pays the others. */
  penalty: number;
  /** Only the immediate variant is held though the full one would be valid (§2.8). */
  stillPenalty: boolean;
}

/** §2.8 Validity: the one send a stored standby offers now, from EP 5's `full_valid_now`. Absent where
 * the engine would refuse it or an amount is unknown (D14): an immediate variant whose penalty is null
 * gets none. */
export function standbySend(s: Standby, fullValidNow: boolean, mine: boolean): StandbySend | null {
  if (s.full && fullValidNow) return { variant: "full", label: "sendFull", cap: mine ? s.fee : 0, penalty: 0, stillPenalty: false };
  if (!s.immediate || s.penalty === null) return null;
  const cap = mine ? s.fee + s.penalty : 0;
  if (fullValidNow) return { variant: "immediate", label: "sendIt", cap, penalty: s.penalty, stillPenalty: true };
  return { variant: "immediate", label: "sendImmediate", cap, penalty: s.penalty, stillPenalty: false };
}

export interface KeyLine {
  field: "operator" | "voting";
  from: string;
  to: string;
  changes: boolean;
}

/** A key change's keys, current → requested, against the list now (§2.6). */
export function keyChangeLines(s: SharedSession): KeyLine[] {
  const k = s.keyChange;
  if (!k) return [];
  const before = k.current ?? s.baseKeys;
  const line = (field: KeyLine["field"], from: string, to: string): KeyLine => ({ field, from, to, changes: from !== to });
  return [line("operator", before?.operatorPublicKey ?? "", k.requested.operatorPublicKey), line("voting", before?.votingAddress ?? "", k.requested.votingAddress)];
}
