// The Governance tab's view model (MG-09, UX-SPEC §4.25.3–4.25.4): pure functions from the
// `governance_*` records to what the screens show. Copy is MG-00's; the D-rows cited are
// docs/design/MG-09-governance-ui.md's unless they say "MG-00" or "MG-06".

import { isAppError } from "../api/backend";
import type { GovernanceInfo } from "../api/generated/GovernanceInfo";
import type { GovernanceSyncState } from "../api/generated/GovernanceSyncState";
import type { MyProposal } from "../api/generated/MyProposal";
import type { ProposalDraft } from "../api/generated/ProposalDraft";
import type { ProposalFieldName } from "../api/generated/ProposalFieldName";
import type { ProposalPreview } from "../api/generated/ProposalPreview";
import type { ProposalRow } from "../api/generated/ProposalRow";
import type { ProposalStatusName } from "../api/generated/ProposalStatusName";
import type { SuperblockDate } from "../api/generated/SuperblockDate";
import type { VoteCast } from "../api/generated/VoteCast";
import type { VoteOutcomeName } from "../api/generated/VoteOutcomeName";
import type { VoteTally } from "../api/generated/VoteTally";
import type { VotingMasternode } from "../api/generated/VotingMasternode";
import { formatLocale, messageTable, t } from "../i18n";
import { numberFormat } from "../i18n/core";
import { masternodes } from "../i18n/catalog/masternodes";

/** Core's `GOVERNANCE_UPDATE_MIN`: one vote change per masternode per hour (D11). */
const VOTE_CHANGE_SECS = 3600;
/** The network accepts a proposal fully at 6 confirmations (MG-00 D16). */
const FULL_CONFIRMATIONS = 6;
/** The proposal form's choices (UX-SPEC §4.25.3). */
export const MAX_PAYMENTS = 12;
export const SUPERBLOCK_CHOICES = 12;
/** Core's proposal name rule as MG-00 words it: 1–40 of `a-z 0-9 - _`. */
const NAME_PATTERN = /^[a-z0-9_-]{1,40}$/;

export const GovText = messageTable("masternodes.governance", [
  "title",
  "create",
  "more",
  "showEnded",
  "hideEnded",
  "numbers",
  "search",
  "sortBy",
  "disabled",
  "disabledHelp",
  "turnOn",
  "turnOnFailed",
  "superblocksFailed",
  "failed",
  "tryAgain",
  "syncNote",
  "browsingOnly",
  "browsingOnlyText",
  "watchOnly",
  "watchOnlyText",
  "voteWatchOnly",
  "createWatchOnly",
  "noVotingKey",
  "emptyMine",
  "emptyNeedsVote",
  "mineNote",
  "linkNote",
  "privacy",
  "voteNote",
  "voteCounts",
  "feeAfter",
  "firstPaymentHelp",
  "nameHelp",
  "linkHelp",
  "submitting",
]);

// ---------------------------------------------------------------- times

const SHORT_DATE: Intl.DateTimeFormatOptions = { month: "short", day: "numeric" };
const LONG_DATE: Intl.DateTimeFormatOptions = { month: "short", day: "numeric", year: "numeric" };
const HOUR: Intl.DateTimeFormatOptions = { hour: "numeric" };
const HOUR_MINUTE: Intl.DateTimeFormatOptions = { hour: "numeric", minute: "2-digit" };
const dateFormats = new Map<Intl.DateTimeFormatOptions, Map<string | undefined, Intl.DateTimeFormat>>();

/** `secs` formatted with `options` for `locale`, with one formatter per pair: a governance render
 * formats many dates, and building a formatter is slow in WebKit. */
function formatDate(secs: number, locale: string | undefined, options: Intl.DateTimeFormatOptions): string {
  let byLocale = dateFormats.get(options);
  if (!byLocale) dateFormats.set(options, (byLocale = new Map()));
  let format = byLocale.get(locale);
  if (!format) byLocale.set(locale, (format = new Intl.DateTimeFormat(locale, options)));
  return format.format(secs * 1000);
}

/** "Nov 4". */
export function shortDate(secs: number, locale?: string): string {
  return formatDate(secs, locale, SHORT_DATE);
}

/** "Feb 1, 2027". */
function longDate(secs: number, locale?: string): string {
  return formatDate(secs, locale, LONG_DATE);
}

/** "3 PM" or "3:30 PM". */
function hourOf(secs: number, locale?: string): string {
  return formatDate(secs, locale, new Date(secs * 1000).getMinutes() === 0 ? HOUR : HOUR_MINUTE);
}

/** "Oct 31 at 2:02 PM". */
export function dateTime(secs: number, locale?: string): string {
  return t("masternodes.governance.dateTime", { date: shortDate(secs, locale), time: formatDate(secs, locale, HOUR_MINUTE) });
}


/** Whole days, hours and minutes of a span, at least zero. */
function splitSpan(secs: number) {
  const s = Math.max(0, Math.round(secs));
  return { days: Math.floor(s / 86_400), hours: Math.floor((s % 86_400) / 3600), minutes: Math.floor((s % 3600) / 60) };
}

/** A span the long way, "1 day 6 hours", "40 minutes", at most two parts. */
export function spanLong(secs: number): string {
  return span("long", secs);
}

/** A span in `style`'s units, at most two parts, at least one minute. */
function span(style: "long" | "short", secs: number): string {
  const { days, hours, minutes } = splitSpan(secs);
  const part = (unit: "days" | "hours" | "minutes", count: number) => t(`masternodes.governance.${style}.${unit}`, { count });
  const pair = (larger: string, smaller: string) => t(`masternodes.governance.${style}.pair`, { larger, smaller });
  if (days > 0) return hours > 0 ? pair(part("days", days), part("hours", hours)) : part("days", days);
  if (hours > 0) return minutes > 0 ? pair(part("hours", hours), part("minutes", minutes)) : part("hours", hours);
  return part("minutes", Math.max(1, minutes));
}

/** The status bar's short span, "1 d 6 h", "40 min". */
export function spanShort(secs: number): string {
  return span("short", secs);
}

/** "40 min ago", "2 hours ago", "Oct 31". */
export function ago(secs: number, now: number, locale?: string): string {
  const d = Math.max(0, now - secs);
  if (d < 60) return t("masternodes.governance.justNow");
  if (d < 3600) return t("masternodes.governance.minutesAgo", { count: Math.floor(d / 60) });
  if (d < 86_400) return t("masternodes.governance.hoursAgo", { count: Math.floor(d / 3600) });
  return shortDate(secs, locale);
}

// ---------------------------------------------------------------- the clock card

export interface ClockView {
  title: string;
  sub: string;
  /** Percent of the cycle behind the tip, and where the estimated cutoff sits. */
  done: number;
  cutoff: number;
  labels: [string, string, string];
  /** The status bar's item (D7), or `null` once the cutoff has passed. */
  statusBar: string | null;
  statusTooltip: string;
}

const clamp = (n: number) => Math.min(100, Math.max(0, n));
/** A count, grouped for the display language. */
const fmt = (n: number) => numberFormat(formatLocale(undefined)).format(n);

/** §4.25.3's clock card, from `info` (its clock plus the cycle). Times are the engine's ETAs,
 * always "about" (MG-02 D9). */
export function clockView(info: GovernanceInfo, now: number, locale?: string): ClockView {
  const c = info.clock;
  const payout = shortDate(c.superblockEta, locale);
  const start = c.nextSuperblock - info.superblockCycle;
  const at = (height: number) => clamp(((height - start) / info.superblockCycle) * 100);
  const spacing = (c.superblockEta - now) / Math.max(1, c.blocksToSuperblock);
  const lastAt = c.superblockEta - info.superblockCycle * spacing;
  const labels: [string, string, string] = [
    c.lastSuperblock > 0 ? t("masternodes.governance.clock.lastPayout", { date: shortDate(lastAt, locale) }) : t("masternodes.governance.clock.cycleStart"),
    t("masternodes.governance.clock.cutoff", { date: shortDate(c.votingCutoffEstimate, locale) }),
    payout,
  ];
  const block = t("masternodes.governance.clock.block", { height: c.nextSuperblock });
  const statusTooltip = t("masternodes.governance.clock.tooltip", { cutoff: c.votingCutoffHeight, payout, next: c.nextSuperblock });
  const base = { done: at(c.tipHeight), cutoff: at(c.votingCutoffHeight), labels, statusTooltip };
  if (c.blocksToSuperblock === 0) {
    return { ...base, title: t("masternodes.governance.clock.dueTitle", { block }), sub: t("masternodes.governance.clock.dueSub", { payout }), statusBar: null };
  }
  if (c.votingOpen) {
    const left = Math.max(0, c.votingCutoffEstimate - now);
    return {
      ...base,
      title: t("masternodes.governance.clock.openTitle", { left: spanLong(left), payout }),
      sub: t("masternodes.governance.clock.openSub", { payout, hour: hourOf(c.superblockEta, locale), block }),
      statusBar: t("masternodes.governance.clock.statusBar", { payout, left: spanShort(left) }),
    };
  }
  return {
    ...base,
    title: t("masternodes.governance.clock.lateTitle", { payout }),
    sub: t("masternodes.governance.clock.lateSub", { left: spanLong(c.superblockEta - now), block }),
    statusBar: null,
  };
}

// ---------------------------------------------------------------- the budget card

export interface BudgetView {
  title: string;
  /** Synced: the passing total and the part that doesn't fit, in duffs, with the bar's widths. */
  synced: { allocated: number; passing: number; overCount: number; overAmount: number; passingPct: number; overPct: number } | null;
  budget: number;
}

/** §4.25.3's budget card: exact budget, vote-derived figures only once synced (MG-00 D21, D45). */
export function budgetView(info: GovernanceInfo, rows: ProposalRow[] | undefined, locale?: string): BudgetView {
  const title = t("masternodes.governance.budget.title", { date: shortDate(info.clock.superblockEta, locale) });
  const counts = info.proposals;
  if (!counts || info.allocatedEstimate === null || !rows) return { title, synced: null, budget: info.budget };
  const over = rows.filter((r) => r.status === "unfunded" && !isEnded(r));
  const overAmount = over.reduce((sum, r) => sum + r.paymentAmount, 0);
  const pct = (n: number) => (info.budget > 0 ? clamp((n / info.budget) * 100) : 0);
  const passingPct = pct(info.allocatedEstimate);
  return {
    title,
    budget: info.budget,
    synced: {
      allocated: info.allocatedEstimate,
      passing: counts.passing,
      overCount: over.length,
      overAmount,
      passingPct,
      overPct: Math.min(100 - passingPct, pct(overAmount)),
    },
  };
}

// ---------------------------------------------------------------- the sync state

export type ListState =
  | { kind: "disabled" }
  | { kind: "syncing"; line: string }
  | { kind: "failed"; text: string }
  | { kind: "ready" };

/** Where the list area is (MG-00 D21): the engine's phase, with progress as counts. */
export function listState(sync: GovernanceSyncState): ListState {
  switch (sync.phase) {
    case "disabled":
      return { kind: "disabled" };
    case "failed":
      return { kind: "failed", text: GovText.failed };
    case "synced":
      return { kind: "ready" };
    case "waiting":
      return { kind: "syncing", line: t("masternodes.governance.sync.waiting") };
    case "syncingObjects":
      return { kind: "syncing", line: `${t("masternodes.governance.sync.objects", { objects: sync.objects })} ${GovText.syncNote}` };
    case "syncingVotes":
      return { kind: "syncing", line: `${t("masternodes.governance.sync.votes", { objects: sync.objects, votes: sync.votes })} ${GovText.syncNote}` };
  }
}

// ---------------------------------------------------------------- proposals

export type Tone = "success" | "warning" | "info" | "danger" | "neutral";
export interface ChipView {
  text: string;
  tone: Tone;
  tooltip: string;
}

const TONE: Record<ProposalStatusName, Tone> = {
  passing: "success",
  unfunded: "warning",
  voting: "info",
  failing: "danger",
  funded: "success",
  confirming: "neutral",
  pending: "neutral",
  lapsed: "neutral",
};

type ChipName = keyof typeof masternodes.governance.chip;

/** A chip whose text and tooltip are the catalog's `masternodes.governance.chip.<name>`. */
const chip = (name: ChipName, tone: Tone): ChipView => ({
  get text() {
    return t(`masternodes.governance.chip.${name}.text`);
  },
  tone,
  get tooltip() {
    return t(`masternodes.governance.chip.${name}.tooltip`);
  },
});
const STATUS_CHIPS = Object.fromEntries(Object.entries(TONE).map(([status, tone]) => [status, chip(status as ProposalStatusName, tone)])) as Record<ProposalStatusName, ChipView>;
const CLOSE_CALL = chip("closeCall", "neutral");
const RANGED: ReadonlySet<ProposalStatusName> = new Set(["passing", "unfunded", "voting", "failing"]);

/** MG-00 D17 with D45's Too close to call (D10). */
export function statusChip(status: ProposalStatusName, closeCall = false): ChipView {
  return closeCall && RANGED.has(status) ? CLOSE_CALL : STATUS_CHIPS[status];
}

export const isEnded = (r: Pick<ProposalRow, "status">) => r.status === "lapsed";

/** The net yes range over the votes whose weight the engine can't pin down (D10). */
export function netRange(t: VoteTally): [number, number] {
  return [t.yes.min - t.no.max, t.yes.max - t.no.min];
}

const signed = (n: number) => (n > 0 ? `+${fmt(n)}` : n < 0 ? `−${fmt(-n)}` : fmt(0));

/** "about +414 over", "about 692 short", "about 6 short; −18 to +6" (MG-00 D45). */
export function marginText(row: Pick<ProposalRow, "tally" | "closeCall" | "status">, threshold: number): string | null {
  if (!RANGED.has(row.status)) return null;
  const m = row.tally.netYesEstimate - threshold;
  const base = m >= 0 ? t("masternodes.governance.marginOver", { margin: signed(m) }) : t("masternodes.governance.marginShort", { margin: fmt(-m) });
  if (!row.closeCall) return base;
  const [lo, hi] = netRange(row.tally);
  return t("masternodes.governance.marginRange", { margin: base, low: signed(lo - threshold), high: signed(hi - threshold) });
}

/** The vote bar's widths in percent of the voting weight, and the funding line's position. */
export function voteBar(t: VoteTally, threshold: number, weight: number) {
  const of = (n: number) => (weight > 0 ? clamp((n / weight) * 100) : 0);
  return { yes: of(t.yes.estimate), no: of(t.no.estimate), abstain: of(t.abstain.estimate), line: of(threshold) };
}

/** "1,500 Ð × 3 payments" or "… once": the part after the amount. */
export function paymentsSuffix(count: number): string {
  return count === 1 ? t("masternodes.governance.once") : t("masternodes.governance.times", { count });
}

/** The detail pane's line beside the chip (§4.25.3). */
export function paneNote(row: ProposalRow, info: GovernanceInfo | undefined, locale?: string): string {
  const payout = info ? shortDate(info.clock.superblockEta, locale) : null;
  if (row.closeCall && RANGED.has(row.status)) return t("masternodes.governance.note.closeCall");
  switch (row.status) {
    case "passing":
      return payout ? t("masternodes.governance.note.passing", { date: payout }) : t("masternodes.governance.note.passingNext");
    case "unfunded":
      return t("masternodes.governance.note.unfunded");
    case "voting":
      return info
        ? t("masternodes.governance.note.voting", { votes: Math.max(0, info.passingThreshold - row.tally.netYesEstimate) })
        : t("masternodes.governance.note.votingUnknown");
    case "failing":
      return t("masternodes.governance.note.failing");
    case "funded":
      return t("masternodes.governance.note.funded");
    case "confirming":
    case "pending":
      return t("masternodes.governance.note.confirming");
    case "lapsed":
      return t("masternodes.governance.note.lapsed");
  }
}

/** "1,500 Ð × 3 payments, Nov 4 to Jan 3": the amount is the caller's, this is the rest. */
export function askSpan(row: Pick<ProposalRow, "paymentsRequested" | "startEpoch" | "endEpoch">, locale?: string): string {
  return t("masternodes.governance.askSpan", { payments: paymentsSuffix(row.paymentsRequested), from: shortDate(row.startEpoch, locale), to: shortDate(row.endEpoch, locale) });
}

/** The pane's weighted votes: the headline, the legend and the range line (MG-00 D45). */
export function votesView(tally: VoteTally, threshold: number) {
  const [lo, hi] = netRange(tally);
  return {
    head: t("masternodes.governance.votes.head", { net: tally.netYesEstimate, threshold }),
    legend: {
      yes: t("masternodes.governance.votes.yes", { votes: tally.yes.estimate }),
      no: t("masternodes.governance.votes.no", { votes: tally.no.estimate }),
      abstain: t("masternodes.governance.votes.abstain", { votes: tally.abstain.estimate }),
    },
    range: lo === hi ? t("masternodes.governance.votes.exact") : t("masternodes.governance.votes.range", { low: lo, high: hi }),
  };
}

/** "XqP2…7mVd". */
export const shortAddress = (a: string) => (a.length > 12 ? `${a.slice(0, 4)}…${a.slice(-4)}` : a);

/** A link shown without its scheme and `www.` (D9). */
export const linkText = (url: string) => url.replace(/^https?:\/\//, "").replace(/^www\./, "");

// ---------------------------------------------------------------- the wallet's own votes

export const shortHash = (hash: string) => hash.slice(0, 8);

/** The wallet's label and the list's service of a masternode, by ProTx hash (MG-09 F3). */
export type NodeNames = ReadonlyMap<string, { label: string | null; service: string | null }>;

/** "Home node · 203.0.113.7:19999", else "Masternode 1a2b3c4d" (D4, F3). */
export function nodeName(n: Pick<VotingMasternode, "proTxHash" | "kind">, names?: NodeNames): string {
  const known = names?.get(n.proTxHash);
  const name =
    known?.label ?? t(n.kind === "evonode" ? "masternodes.governance.evonodeNamed" : "masternodes.governance.masternodeNamed", { hash: shortHash(n.proTxHash) });
  return known?.service ? t("masternodes.governance.withService", { name, service: known.service }) : name;
}

export const outcomeText = (o: VoteOutcomeName) => t(`masternodes.governance.outcome.${o}`);

/** A masternode's own vote seen, or for a shared key the key's (mg-engine §2.4, D8). */
function seenVote(n: VotingMasternode): { outcome: VoteOutcomeName; time: number | null; shared: boolean } | null {
  if (n.currentVote) return { outcome: n.currentVote, time: n.voteTime, shared: false };
  if (n.keyHolders > 1 && n.keyLastVote) return { outcome: n.keyLastVote, time: n.keyLastVoteTime, shared: true };
  return null;
}

/** "You: Yes (last seen)", "You: no vote seen", "You: 1 of 3 seen voting" (MG-00 D45); `null`
 * without a valid voting masternode. */
export function youSummary(nodes: VotingMasternode[] | undefined): string | null {
  const valid = (nodes ?? []).filter((n) => n.valid);
  if (valid.length === 0) return null;
  const seen = valid.map(seenVote).filter((v) => v !== null);
  if (seen.length === 0) return t("masternodes.governance.you.none");
  if (valid.length === 1) return t("masternodes.governance.you.one", { outcome: outcomeText(seen[0].outcome) });
  return t("masternodes.governance.you.some", { seen: seen.length, total: valid.length });
}

/** Open, with a valid masternode of this wallet that has no vote seen (D8). */
export function needsMyVote(row: Pick<ProposalRow, "status">, nodes: VotingMasternode[] | undefined): boolean {
  if (isEnded(row) || row.status === "funded") return false;
  return (nodes ?? []).some((n) => n.valid && seenVote(n) === null);
}

/** The detail pane's "Your masternodes" line for one masternode. */
export function lastVoteLine(n: VotingMasternode, now: number, locale?: string): string {
  const v = seenVote(n);
  if (!v) return t("masternodes.governance.lastVote.none");
  const outcome = outcomeText(v.outcome);
  if (v.shared) return t("masternodes.governance.lastVote.shared", { outcome, count: n.keyHolders });
  return v.time === null ? t("masternodes.governance.lastVote.undated", { outcome }) : t("masternodes.governance.lastVote.dated", { outcome, ago: ago(v.time, now, locale) });
}

// ---------------------------------------------------------------- the vote sheet

export interface NodeRowView {
  proTxHash: string;
  name: string;
  sub: string;
  weight: number;
  lastSeen: string;
  eligible: boolean;
}

/** §4.25.4's rows (D4, D11): eligible rows start checked; a banned masternode and one that voted
 * less than an hour ago are disabled with the reason. */
export function voteRows(nodes: VotingMasternode[], now: number, locale?: string, names?: NodeNames): NodeRowView[] {
  return nodes.map((n) => {
    const v = seenVote(n);
    const base = { proTxHash: n.proTxHash, name: nodeName(n, names), weight: n.weight, lastSeen: v ? outcomeText(v.outcome) : t("masternodes.governance.node.none") };
    if (!n.valid) return { ...base, sub: t("masternodes.governance.node.banned"), eligible: false, lastSeen: "—" };
    // `vote` refuses it until the collateral is found (MG-02 D80).
    if (!n.collateralResolved) return { ...base, sub: rejectedText(COLLATERAL_UNRESOLVED), eligible: false };
    if (v && !v.shared && v.time !== null && now - v.time < VOTE_CHANGE_SECS) {
      const after = formatDate(v.time + VOTE_CHANGE_SECS, locale, HOUR_MINUTE);
      return { ...base, sub: t("masternodes.governance.node.recent", { outcome: outcomeText(v.outcome), ago: ago(v.time, now, locale), time: after }), eligible: false };
    }
    const custody = n.key.kind === "keychain" ? t("masternodes.governance.node.keychain", { index: n.key.index + 1 }) : t("masternodes.governance.node.added");
    const sub = n.keyHolders > 1 ? t("masternodes.governance.node.eligibleShared", { custody, count: n.keyHolders }) : custody;
    return { ...base, sub, eligible: true };
  });
}

/** The outcome a sheet starts with: the last vote seen when every row that can vote saw the same one
 * (§4.25.4), else none. */
export function presetOutcome(nodes: VotingMasternode[]): VoteOutcomeName | null {
  const seen = new Set(nodes.filter((n) => n.valid && n.collateralResolved).map((n) => seenVote(n)?.outcome ?? null));
  return seen.size === 1 ? [...seen][0] : null;
}

/** "2 masternodes · weight 8 · no fee", for the checked eligible rows. */
export function selectionNote(picked: Pick<NodeRowView, "weight">[]): string {
  const weight = picked.reduce((s, r) => s + r.weight, 0);
  return t("masternodes.governance.selection", { count: picked.length, weight });
}

/** The engine's refusal of a PoSe-banned masternode's vote (`VoteRejection::Banned`, Core's wording). */
export const BANNED_REJECTION = "masternode_not_found";

/** The engine's refusal of a vote whose masternode's collateral isn't resolved yet (MG-02 D80, MG-05 §5). */
export const COLLATERAL_UNRESOLVED = "collateral_unresolved";

const REJECTED = messageTable("masternodes.governance.rejected", Object.keys(masternodes.governance.rejected));

/** A per-masternode or `vote_rejected` reason as a sentence (D13). */
export function rejectedText(reason: string): string {
  return Object.hasOwn(REJECTED, reason) ? REJECTED[reason] : t("masternodes.governance.refusedVote");
}

export interface VoteResultView {
  title: string;
  line: string;
  rows: { proTxHash: string; name: string; sub: string; sent: boolean }[];
}

/** §4.25.4's result: "1 of 2 votes sent", "Yes · weight 4 of 8", a line per masternode. */
export function voteResult(casts: VoteCast[], nodes: VotingMasternode[], outcome: VoteOutcomeName, names?: NodeNames): VoteResultView {
  const byHash = new Map(nodes.map((n) => [n.proTxHash, n]));
  const weightOf = (h: string) => byHash.get(h)?.weight ?? 1;
  const sent = casts.filter((c) => c.rejected === null);
  const total = casts.reduce((s, c) => s + weightOf(c.proTxHash), 0);
  const sentWeight = sent.reduce((s, c) => s + weightOf(c.proTxHash), 0);
  return {
    title: t("masternodes.governance.result.title", { sent: sent.length, count: casts.length }),
    line: t("masternodes.governance.result.line", { outcome: outcomeText(outcome), sent: sentWeight, total }),
    rows: casts.map((c) => {
      const n = byHash.get(c.proTxHash);
      const name = n ? nodeName(n, names) : t("masternodes.governance.masternodeNamed", { hash: shortHash(c.proTxHash) });
      if (c.rejected !== null) return { proTxHash: c.proTxHash, name, sub: rejectedText(c.rejected), sent: false };
      const peers = c.peers === 0 ? t("masternodes.governance.result.reconnects") : t("masternodes.governance.result.peers", { count: c.peers });
      return { proTxHash: c.proTxHash, name, sub: t("masternodes.governance.result.sent", { weight: weightOf(c.proTxHash), peers }), sent: true };
    }),
  };
}

// ---------------------------------------------------------------- errors

const REFUSAL = messageTable("masternodes.governance.refusal", Object.keys(masternodes.governance.refusal));
const refusalText = (reason: string) => (Object.hasOwn(REFUSAL, reason) ? REFUSAL[reason] : t("masternodes.governance.errors.submitRefused"));

/** A governance call's error as a sentence (D13); `null` when it is not a governance code, so
 * the caller can try the send sentences (`screens/pay/model.ts` `failureText`). */
export function governanceErrorText(e: unknown): string | null {
  if (!isAppError(e)) return null;
  switch (e.code) {
    case "governance.sync_disabled":
      return t("masternodes.governance.errors.syncDisabled");
    case "governance.not_synced":
      return t("masternodes.governance.errors.notSynced");
    case "governance.proposal_not_found":
      return t("masternodes.governance.errors.proposalNotFound");
    case "governance.vote_rejected":
      return rejectedText(e.data?.kind === "reason" ? e.data.reason : "");
    case "governance.no_peers":
      return t("masternodes.governance.errors.noPeers");
    case "governance.list_unavailable":
      return t("masternodes.governance.errors.listUnavailable");
    case "governance.invalid_proposal":
      return t("masternodes.governance.errors.invalidProposal");
    case "governance.submit_refused":
      return refusalText(e.data?.kind === "reason" ? e.data.reason : "");
    case "governance.proposal_in_progress":
      return t("masternodes.governance.errors.inProgress");
    case "governance.not_implemented":
      return t("masternodes.governance.errors.notImplemented");
    default:
      return null;
  }
}

/** `governance.proposal_in_progress` (a draft named as one of the wallet's proposals in progress): the
 *  hash of the proposal it names, `null` when it names none (that create call is still out), and
 *  `undefined` for any other error. */
export function inProgressRefusal(e: unknown): string | null | undefined {
  if (!isAppError(e) || e.code !== "governance.proposal_in_progress") return undefined;
  return e.data?.kind === "proposalHash" ? e.data.hash : null;
}

// ---------------------------------------------------------------- the fee (D29, D33, D34)

/** The least margin the spend grant gives over the network fee, as dwcli's
 *  `PROPOSAL_FEE_MARGIN`: the engine picks the coins again at create time (D29). */
const NETWORK_FEE_MARGIN = 10_000;

type FeeFigures = Pick<ProposalPreview, "fee" | "networkFee">;

/** What the user pays in all: the 1 DASH fee (burnt) and the network fee. */
export const feeTotal = (p: FeeFigures): number => p.fee + p.networkFee;

/** The spend grant's cap for the fee the user confirmed: its total and the margin, no more. */
export const feeGrantCap = (p: FeeFigures): number => feeTotal(p) + Math.max(p.networkFee, NETWORK_FEE_MARGIN);

/** Whether Pay's re-read shows the figures the review showed (D33). */
export const sameFee = (a: FeeFigures, b: FeeFigures): boolean => a.fee === b.fee && a.networkFee === b.networkFee;

/** The wallet's own proposal `create_proposal` made from `draft`, if it made one: after a failed
 *  call the fee may be out, so Pay waits for this (D34). `known` are the proposals Mine held
 *  before the call; an ended or failed one isn't this call's either. */
export function createdFrom(mine: MyProposal[], draft: ProposalDraft, known: ReadonlySet<string> = new Set()): MyProposal | undefined {
  return mine.find(
    (p) =>
      !known.has(p.hash) &&
      inProgress(p) &&
      p.name === draft.name &&
      p.url === draft.url &&
      p.paymentAddress === draft.paymentAddress &&
      p.paymentAmount === draft.paymentAmount &&
      p.paymentCount === draft.paymentCount,
  );
}

// ---------------------------------------------------------------- Mine (MG-06)

export interface MineRowView {
  chip: ChipView;
  meta: string | null;
  sub: string;
  /** MG-06 D24: [Submit] shows. */
  submit: boolean;
  /** The failure behind [Submit], a sentence (D15). */
  error: string | null;
  /** Needs the user: counts for the badge (D5). */
  attention: boolean;
  /** `ready_to_submit{sync_disabled}`: offer Turn on (D16). */
  turnOn: boolean;
}

/** `last_error` is the engine's send failure text (MG-06 D10). The engine keeps one only where the
 *  fee may be out already (a maybe-sent create, any resend failure), so every one says so and
 *  points to Submit, which sends the same fee again; never a new proposal (D35). */
function lastErrorText(p: MyProposal): string | null {
  if (!p.lastError) return null;
  const why = p.lastError.replace(/^send: /, "");
  return why.startsWith("broadcast outcome unknown")
    ? t("masternodes.governance.feeMaybeUnsent")
    : t("masternodes.governance.feeMaybeUnsentWhy", { why });
}

/** The toast once `create_proposal` returned: paid, or kept with its fee unsent (MG-06 D10). */
export function createdToast(p: MyProposal): string {
  return p.state.kind === "feeUnsent" || p.state.kind === "feeFailed"
    ? t("masternodes.governance.createdUnsent", { name: p.name })
    : t("masternodes.governance.createdPaid", { name: p.name });
}

/** One Mine row (MG-00 §4.25.3 Mine, MG-06 D24, D15–D16). */
export function mineRow(p: MyProposal, locale?: string): MineRowView {
  const created = dateTime(p.createdAt, locale);
  const s = p.state;
  const none = { meta: null, submit: false, error: null, attention: false, turnOn: false };
  switch (s.kind) {
    case "feeUnsent": {
      const error = lastErrorText(p);
      return {
        ...none,
        chip: chip("feeNotSent", "warning"),
        sub: t("masternodes.governance.mine.unsent", { date: created }),
        submit: true,
        error,
        attention: error !== null,
      };
    }
    case "feePaid":
      return {
        ...none,
        chip: STATUS_CHIPS.pending,
        meta: t("masternodes.governance.mine.of", { done: String(Math.min(s.confirmations, 1)), total: "1" }),
        sub:
          s.confirmations === 0
            ? t("masternodes.governance.mine.paidWaiting", { date: created })
            : t("masternodes.governance.mine.paidConfirmed", { date: created }),
        submit: s.confirmations === 0,
        error: s.confirmations === 0 ? lastErrorText(p) : null,
      };
    case "readyToSubmit":
      if (s.reason === "noPeerAccepted") {
        return {
          ...none,
          chip: chip("readyNoPeer", "warning"),
          sub: t("masternodes.governance.mine.paid", { date: created }),
          submit: true,
          error: t("masternodes.governance.mine.noPeer"),
          attention: true,
        };
      }
      return {
        ...none,
        chip: chip("readyWaiting", "warning"),
        sub: s.reason === "syncDisabled" ? t("masternodes.governance.mine.syncOff") : t("masternodes.governance.mine.waitingSync"),
        turnOn: s.reason === "syncDisabled",
      };
    case "confirming":
      return {
        ...none,
        chip: STATUS_CHIPS.confirming,
        meta: t("masternodes.governance.mine.of", { done: String(Math.min(s.confirmations, FULL_CONFIRMATIONS)), total: String(FULL_CONFIRMATIONS) }),
        sub: t("masternodes.governance.mine.confirming", { count: String(FULL_CONFIRMATIONS) }),
      };
    case "public":
      return {
        ...none,
        chip: s.status ? statusChip(s.status) : chip("submitted", "neutral"),
        sub: p.submittedAt !== null ? t("masternodes.governance.mine.submittedAt", { date: dateTime(p.submittedAt, locale) }) : t("masternodes.governance.mine.created", { date: created }),
      };
    case "feeFailed":
      return {
        ...none,
        chip: chip("feeFailed", "danger"),
        sub: t("masternodes.governance.mine.feeFailed"),
      };
    case "expired":
      return { ...none, chip: STATUS_CHIPS.lapsed, sub: t("masternodes.governance.mine.created", { date: created }) };
  }
}

/** The sidebar badge's governance part (MG-00 D8, D5). */
export function badgeCount(mine: MyProposal[] | undefined, locale?: string): number {
  return (mine ?? []).filter((p) => mineRow(p, locale).attention).length;
}

/** "A proposal in progress": a proposal that has not ended or failed. */
export const inProgress = (p: MyProposal) => p.state.kind !== "expired" && p.state.kind !== "feeFailed";

// ---------------------------------------------------------------- list filters

export type ListFilter = "all" | "needsVote" | "mine";
export type SortOrder = "closest" | "mostAsked" | "newest" | "name";
export const SORTS: { id: SortOrder; title: string }[] = (["closest", "mostAsked", "newest", "name"] as const).map((id) => ({
  id,
  get title() {
    return t(`masternodes.governance.sort.${id}`);
  },
}));

/** §4.25.3's sorts; "closest" keeps the engine's order (status groups, then net yes). */
export function sortRows(rows: ProposalRow[], sort: SortOrder): ProposalRow[] {
  const out = [...rows];
  if (sort === "mostAsked") out.sort((a, b) => b.paymentAmount * b.paymentsRequested - a.paymentAmount * a.paymentsRequested);
  else if (sort === "newest") out.sort((a, b) => b.startEpoch - a.startEpoch);
  else if (sort === "name") out.sort((a, b) => a.name.localeCompare(b.name));
  return out;
}

// ---------------------------------------------------------------- Create proposal

export const FIELD_STEP: Record<ProposalFieldName, 1 | 2> = {
  name: 1,
  url: 1,
  payment_address: 2,
  payment_amount: 2,
  payment_count: 2,
  first_payment: 2,
  payload: 1,
};

/** An engine field refusal (`governance.invalid_proposal{fields}`) as the field's message. */
export function fieldText(field: ProposalFieldName, chainName: string): string {
  switch (field) {
    case "name":
      return t("masternodes.governance.field.name");
    case "url":
      return t("masternodes.governance.field.url");
    case "payment_address":
      return t("masternodes.governance.field.paymentAddress", { chain: chainName });
    case "payment_amount":
      return t("masternodes.governance.field.paymentAmount");
    case "payment_count":
      return t("masternodes.governance.field.paymentCount", { max: String(MAX_PAYMENTS) });
    case "first_payment":
      return t("masternodes.governance.field.firstPayment");
    case "payload":
      return t("masternodes.governance.field.payload");
  }
}

/** The local check of step 1, before the engine's (`preview_proposal`). */
export function detailsProblems(name: string, url: string): Partial<Record<"name" | "url", string>> {
  const out: Partial<Record<"name" | "url", string>> = {};
  if (!NAME_PATTERN.test(name)) out.name = fieldText("name", "");
  if (url.trim() === "") out.url = t("masternodes.governance.field.urlEmpty");
  return out;
}

/** The first payment the form starts with: the next superblock, or the one after when the
 * nearest one's estimated cutoff is within two days (MG-06 D13); with its notice. */
export function defaultFirstPayment(superblocks: SuperblockDate[], now: number, locale?: string): { height: number | null; notice: string | null } {
  if (superblocks.length === 0) return { height: null, notice: null };
  const [first, second] = superblocks;
  if (!first.cutoffSoon || !second) return { height: first.height, notice: null };
  const left = spanLong(Math.max(0, first.votingCutoffEstimate - now));
  return {
    height: second.height,
    notice: t("masternodes.governance.cutoffSoon", { first: shortDate(first.estimatedTime, locale), left, second: shortDate(second.estimatedTime, locale) }),
  };
}

/** "Dec 4, 2026 (block 2,391,664)". */
export function superblockOption(sb: SuperblockDate, locale?: string): string {
  return t("masternodes.governance.superblock", { date: longDate(sb.estimatedTime, locale), height: sb.height });
}

/** "Last payment Feb 1, 2027." from the chosen superblock and count. */
export function lastPaymentText(superblocks: SuperblockDate[], first: number | null, count: number, locale?: string): string | null {
  const i = superblocks.findIndex((s) => s.height === first);
  if (i < 0) return null;
  if (count === 1) return t("masternodes.governance.onePayment");
  const last = superblocks[i + count - 1];
  if (last) return t("masternodes.governance.lastPayment", { date: longDate(last.estimatedTime, locale) });
  // Past the list: one cycle per payment from the last one known.
  const known = superblocks[superblocks.length - 1];
  const cycle = superblocks.length > 1 ? superblocks[1].estimatedTime - superblocks[0].estimatedTime : 0;
  return t("masternodes.governance.lastPaymentAbout", { date: longDate(known.estimatedTime + cycle * (i + count - superblocks.length), locale) });
}

/** The budget notice of step 2, shown once proposals are synced (§4.25.3, MG-00 D45). */
export function budgetNotice(info: GovernanceInfo | undefined, amount: (duffs: number) => string): string | null {
  if (!info || info.allocatedEstimate === null) return null;
  return t("masternodes.governance.budgetNotice", { budget: amount(info.budget), allocated: amount(info.allocatedEstimate) });
}

// ---------------------------------------------------------------- Governance numbers

/** The rows of Governance numbers… (MG-00 D20, §4.25.3). */
export function numbersRows(info: GovernanceInfo, sync: GovernanceSyncState | undefined, rows: ProposalRow[] | undefined, amount: (d: number) => string, locale?: string) {
  const c = info.clock;
  const regular = info.validMasternodes - info.validEvonodes;
  const N = "masternodes.governance.numbersSheet";
  const cycle = [
    { label: t(`${N}.lastSuperblock`), value: c.lastSuperblock > 0 ? t(`${N}.block`, { height: c.lastSuperblock }) : t(`${N}.noneYet`) },
    {
      label: t(`${N}.cutoff`),
      value: t(`${N}.cutoffValue`, { height: c.votingCutoffHeight, date: shortDate(c.votingCutoffEstimate, locale), payout: shortDate(c.superblockEta, locale) }),
    },
    { label: t(`${N}.nextSuperblock`), value: t(`${N}.nextValue`, { height: c.nextSuperblock, date: shortDate(c.superblockEta, locale) }) },
    { label: t(`${N}.cycleLength`), value: t(`${N}.cycleValue`, { blocks: info.superblockCycle, window: info.maturityWindow }) },
  ];
  const synced = info.proposals !== null && info.allocatedEstimate !== null;
  const open = rows?.filter((r) => !isEnded(r)).length;
  const votes = [
    { label: t(`${N}.weight`), value: t(`${N}.weightValue`, { weight: info.validWeight, regular, evonodes: info.validEvonodes }) },
    { label: t(`${N}.fundingLine`), value: t(`${N}.fundingLineValue`, { votes: info.passingThreshold }) },
    { label: t(`${N}.budget`), value: t(`${N}.budgetValue`, { amount: amount(info.budget) }) },
    {
      label: t(`${N}.passing`),
      value:
        synced && info.proposals
          ? t(`${N}.passingValue`, { amount: amount(info.allocatedEstimate!), passing: info.proposals.passing, open: open ?? info.proposals.total, over: info.proposals.unfunded })
          : t(`${N}.afterSync`),
    },
    {
      label: t(`${N}.data`),
      value: synced ? t(`${N}.dataValue`, { height: c.tipHeight, count: sync?.peers ?? 0 }) : t(`${N}.afterSync`),
    },
  ];
  return { cycle, votes };
}
