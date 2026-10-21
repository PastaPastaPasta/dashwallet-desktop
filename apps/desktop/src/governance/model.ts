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

/** Core's `GOVERNANCE_UPDATE_MIN`: one vote change per masternode per hour (D11). */
const VOTE_CHANGE_SECS = 3600;
/** The network accepts a proposal fully at 6 confirmations (MG-00 D16). */
const FULL_CONFIRMATIONS = 6;
/** The proposal form's choices (UX-SPEC §4.25.3). */
export const MAX_PAYMENTS = 12;
export const SUPERBLOCK_CHOICES = 12;
/** Core's proposal name rule as MG-00 words it: 1–40 of `a-z 0-9 - _`. */
const NAME_PATTERN = /^[a-z0-9_-]{1,40}$/;

export const GovText = {
  title: "Masternodes",
  create: "Create proposal",
  more: "More",
  showEnded: "Show ended proposals",
  hideEnded: "Hide ended proposals",
  numbers: "Governance numbers…",
  search: "Search proposals",
  sortBy: "Sort by",
  disabled: "Governance data is off for this network.",
  disabledHelp: "Turn it on to load proposals and votes from the network.",
  turnOn: "Turn on",
  turnOnFailed: "Couldn't turn on governance data.",
  superblocksFailed: "Couldn't read the next superblocks.",
  failed: "Couldn't load proposals: no peer is serving governance data.",
  tryAgain: "Try again",
  syncNote: "Counts appear when the sync is complete, never partly.",
  browsingOnly: "Browsing only.",
  browsingOnlyText: "This wallet holds no masternode voting keys, so it can read proposals but not vote. You can still create a proposal.",
  watchOnly: "Watch-only wallet.",
  watchOnlyText: "It shows proposals and the votes peers sent. Voting and creating a proposal need the wallet that holds the keys.",
  voteWatchOnly: "Voting needs the wallet that holds the voting keys",
  createWatchOnly: "Creating a proposal needs a wallet that can sign transactions",
  noVotingKey: "No masternode in this wallet can vote on this proposal",
  emptyMine: "You haven't created a proposal.",
  emptyNeedsVote: "You have voted on every open proposal.",
  mineNote: "Proposals you create are kept with this wallet, so you can finish them after a restart; they travel in wallet backups.",
  linkNote: "Copy the link to open it in your browser.",
  privacy: "Voting with several masternodes together shows that one person controls them. Choose fewer to keep them unlinked.",
  voteNote: "A new vote replaces that masternode's earlier one. The network accepts one change per masternode per hour.",
  voteCounts: "The counts update as peers relay the votes.",
  feeWarning: "Creating a proposal pays 1 DASH to the network. This fee is non-refundable regardless of outcome.",
  feeAfter: "After it confirms, the wallet submits the proposal for you. You can close this window; Mine shows the progress.",
  firstPaymentHelp: "Payments happen at superblocks, about every 29 days.",
  nameHelp: "Lower case letters, digits, - and _",
  linkHelp: "A page describing the proposal, such as a Dash Central or forum post",
  submitting: "Sending…",
} as const;

// ---------------------------------------------------------------- times

/** "Nov 4". */
export function shortDate(secs: number, locale?: string): string {
  return new Date(secs * 1000).toLocaleDateString(locale, { month: "short", day: "numeric" });
}

/** "Feb 1, 2027". */
function longDate(secs: number, locale?: string): string {
  return new Date(secs * 1000).toLocaleDateString(locale, { month: "short", day: "numeric", year: "numeric" });
}

/** "3 PM" or "3:30 PM". */
function hourOf(secs: number, locale?: string): string {
  const d = new Date(secs * 1000);
  return d.toLocaleTimeString(locale, d.getMinutes() === 0 ? { hour: "numeric" } : { hour: "numeric", minute: "2-digit" });
}

/** "Oct 31 at 2:02 PM". */
function dateTime(secs: number, locale?: string): string {
  const d = new Date(secs * 1000);
  return `${shortDate(secs, locale)} at ${d.toLocaleTimeString(locale, { hour: "numeric", minute: "2-digit" })}`;
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** Whole days, hours and minutes of a span, at least zero. */
function splitSpan(secs: number) {
  const s = Math.max(0, Math.round(secs));
  return { days: Math.floor(s / 86_400), hours: Math.floor((s % 86_400) / 3600), minutes: Math.floor((s % 3600) / 60) };
}

/** A span the long way, "1 day 6 hours", "40 minutes", at most two parts. */
export function spanLong(secs: number): string {
  const { days, hours, minutes } = splitSpan(secs);
  if (days > 0) return hours > 0 ? `${plural(days, "day")} ${plural(hours, "hour")}` : plural(days, "day");
  if (hours > 0) return minutes > 0 ? `${plural(hours, "hour")} ${plural(minutes, "minute")}` : plural(hours, "hour");
  return plural(Math.max(1, minutes), "minute");
}

/** The status bar's short span, "1 d 6 h", "40 min". */
export function spanShort(secs: number): string {
  const { days, hours, minutes } = splitSpan(secs);
  if (days > 0) return hours > 0 ? `${days} d ${hours} h` : `${days} d`;
  if (hours > 0) return minutes > 0 ? `${hours} h ${minutes} min` : `${hours} h`;
  return `${Math.max(1, minutes)} min`;
}

/** "40 min ago", "2 hours ago", "Oct 31". */
export function ago(secs: number, now: number, locale?: string): string {
  const d = Math.max(0, now - secs);
  if (d < 60) return "just now";
  if (d < 3600) return `${Math.floor(d / 60)} min ago`;
  if (d < 86_400) return `${plural(Math.floor(d / 3600), "hour")} ago`;
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
const fmt = (n: number) => n.toLocaleString("en-US");

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
    c.lastSuperblock > 0 ? `${shortDate(lastAt, locale)} · last payout` : "cycle start",
    `estimated cutoff, ${shortDate(c.votingCutoffEstimate, locale)}`,
    payout,
  ];
  const block = `block ${fmt(c.nextSuperblock)}`;
  const statusTooltip = `Votes cast before about block ${fmt(c.votingCutoffHeight)} (the estimated cutoff) count for the ${payout} payout at block ${fmt(c.nextSuperblock)}. Later votes still count, but may only change later payouts.`;
  const base = { done: at(c.tipHeight), cutoff: at(c.votingCutoffHeight), labels, statusTooltip };
  if (c.blocksToSuperblock === 0) {
    return { ...base, title: `Payout due now (${block})`, sub: `The ${payout} payout is being made now.`, statusBar: null };
  }
  if (c.votingOpen) {
    const left = Math.max(0, c.votingCutoffEstimate - now);
    return {
      ...base,
      title: `About ${spanLong(left)} left to vote for the ${payout} payout`,
      sub: `Next payout to proposals: ${payout}, around ${hourOf(c.superblockEta, locale)} · ${block}. Later votes may only count for later payouts.`,
      statusBar: `${payout} votes: about ${spanShort(left)} left`,
    };
  }
  return {
    ...base,
    title: `Votes now may be too late for the ${payout} payout`,
    sub: `They still count for later ones. Payout in about ${spanLong(c.superblockEta - now)} · ${block}.`,
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
  const title = `Budget for ${shortDate(info.clock.superblockEta, locale)}`;
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
      return { kind: "syncing", line: "Looking for a peer that serves governance data." };
    case "syncingObjects":
      return { kind: "syncing", line: `${fmt(sync.objects)} proposals and triggers so far. ${GovText.syncNote}` };
    case "syncingVotes":
      return { kind: "syncing", line: `${fmt(sync.objects)} proposals and ${fmt(sync.votes)} votes so far. ${GovText.syncNote}` };
  }
}

// ---------------------------------------------------------------- proposals

export type Tone = "success" | "warning" | "info" | "danger" | "neutral";
export interface ChipView {
  text: string;
  tone: Tone;
  tooltip: string;
}

const STATUS_CHIPS: Record<ProposalStatusName, ChipView> = {
  passing: { text: "Passing", tone: "success", tooltip: "From the votes peers sent: enough net yes votes and fits the budget." },
  unfunded: { text: "Over budget", tone: "warning", tooltip: "From the votes peers sent: enough votes, but proposals with more votes use up the budget." },
  voting: { text: "Needs votes", tone: "info", tooltip: "From the votes peers sent: not enough net yes votes yet." },
  failing: { text: "Not funded", tone: "danger", tooltip: "From the votes peers sent: more no than yes votes." },
  funded: { text: "Funded", tone: "success", tooltip: "Paid in the last superblock." },
  confirming: { text: "Confirming", tone: "neutral", tooltip: "Submitted after the first confirmation; fully valid after six." },
  pending: { text: "Fee paid", tone: "neutral", tooltip: "Its fee is paid; the wallet submits it after the first confirmation." },
  lapsed: { text: "Ended", tone: "neutral", tooltip: "All payments made or the end date passed." },
};
const CLOSE_CALL: ChipView = { text: "Too close to call", tone: "neutral", tooltip: "The vote count is an estimate, and this one is within its margin." };
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

const signed = (n: number) => (n > 0 ? `+${fmt(n)}` : n < 0 ? `−${fmt(-n)}` : "0");

/** "about +414 over", "about 692 short", "about 6 short; −18 to +6" (MG-00 D45). */
export function marginText(row: Pick<ProposalRow, "tally" | "closeCall" | "status">, threshold: number): string | null {
  if (!RANGED.has(row.status)) return null;
  const m = row.tally.netYesEstimate - threshold;
  const base = m >= 0 ? `about ${signed(m)} over` : `about ${fmt(-m)} short`;
  if (!row.closeCall) return base;
  const [lo, hi] = netRange(row.tally);
  return `${base}; ${signed(lo - threshold)} to ${signed(hi - threshold)}`;
}

/** The vote bar's widths in percent of the voting weight, and the funding line's position. */
export function voteBar(t: VoteTally, threshold: number, weight: number) {
  const of = (n: number) => (weight > 0 ? clamp((n / weight) * 100) : 0);
  return { yes: of(t.yes.estimate), no: of(t.no.estimate), abstain: of(t.abstain.estimate), line: of(threshold) };
}

/** "1,500 Ð × 3 payments" or "… once": the part after the amount. */
export function paymentsSuffix(count: number): string {
  return count === 1 ? "once" : `× ${count} payments`;
}

/** The detail pane's line beside the chip (§4.25.3). */
export function paneNote(row: ProposalRow, info: GovernanceInfo | undefined, locale?: string): string {
  const payout = info ? shortDate(info.clock.superblockEta, locale) : null;
  if (row.closeCall && RANGED.has(row.status)) return "within the margin of the votes seen";
  switch (row.status) {
    case "passing":
      return payout ? `paid ${payout} if it stays above the line` : "paid at the next superblock if it stays above the line";
    case "unfunded":
      return "passes the vote, but proposals with more votes use up the budget";
    case "voting":
      return info ? `needs about ${fmt(Math.max(0, info.passingThreshold - row.tally.netYesEstimate))} more net yes votes` : "needs more net yes votes";
    case "failing":
      return "more no than yes votes";
    case "funded":
      return "paid in the last superblock";
    case "confirming":
    case "pending":
      return "its fee is still confirming";
    case "lapsed":
      return "all payments made or the end date passed";
  }
}

/** "1,500 Ð × 3 payments, Nov 4 to Jan 3": the amount is the caller's, this is the rest. */
export function askSpan(row: Pick<ProposalRow, "paymentsRequested" | "startEpoch" | "endEpoch">, locale?: string): string {
  return `${paymentsSuffix(row.paymentsRequested)}, ${shortDate(row.startEpoch, locale)} to ${shortDate(row.endEpoch, locale)}`;
}

/** The pane's weighted votes: the headline, the legend and the range line (MG-00 D45). */
export function votesView(t: VoteTally, threshold: number) {
  const [lo, hi] = netRange(t);
  return {
    head: `about ${fmt(t.netYesEstimate)} net yes · needs ${fmt(threshold)}`,
    legend: { yes: `Yes about ${fmt(t.yes.estimate)}`, no: `No about ${fmt(t.no.estimate)}`, abstain: `Abstain about ${fmt(t.abstain.estimate)}` },
    range: lo === hi ? "From the votes peers sent. Unseen votes can change it." : `From the votes peers sent: ${fmt(lo)} to ${fmt(hi)} net yes. Unseen votes can change it.`,
  };
}

/** "XqP2…7mVd". */
export const shortAddress = (a: string) => (a.length > 12 ? `${a.slice(0, 4)}…${a.slice(-4)}` : a);

/** A link shown without its scheme and `www.` (D9). */
export const linkText = (url: string) => url.replace(/^https?:\/\//, "").replace(/^www\./, "");

// ---------------------------------------------------------------- the wallet's own votes

export const shortHash = (hash: string) => hash.slice(0, 8);

/** "Masternode 1a2b3c4d" (D4: the binding carries no label or service). */
export function nodeName(n: Pick<VotingMasternode, "proTxHash" | "kind">): string {
  return `${n.kind === "evonode" ? "Evonode" : "Masternode"} ${shortHash(n.proTxHash)}`;
}

const OUTCOME: Record<VoteOutcomeName, string> = { yes: "Yes", no: "No", abstain: "Abstain" };
export const outcomeText = (o: VoteOutcomeName) => OUTCOME[o];

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
  if (seen.length === 0) return "You: no vote seen";
  if (valid.length === 1) return `You: ${outcomeText(seen[0].outcome)} (last seen)`;
  return `You: ${seen.length} of ${valid.length} seen voting`;
}

/** Open, with a valid masternode of this wallet that has no vote seen (D8). */
export function needsMyVote(row: Pick<ProposalRow, "status">, nodes: VotingMasternode[] | undefined): boolean {
  if (isEnded(row) || row.status === "funded") return false;
  return (nodes ?? []).some((n) => n.valid && seenVote(n) === null);
}

/** The detail pane's "Your masternodes" line for one masternode. */
export function lastVoteLine(n: VotingMasternode, now: number, locale?: string): string {
  const v = seenVote(n);
  if (!v) return "No vote seen";
  if (v.shared) return `Last vote seen with this shared key: ${outcomeText(v.outcome)} (${n.keyHolders} masternodes use this key)`;
  return v.time === null ? `${outcomeText(v.outcome)} · last vote seen` : `${outcomeText(v.outcome)} · last vote seen ${ago(v.time, now, locale)}`;
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
export function voteRows(nodes: VotingMasternode[], now: number, locale?: string): NodeRowView[] {
  return nodes.map((n) => {
    const v = seenVote(n);
    const base = { proTxHash: n.proTxHash, name: nodeName(n), weight: n.weight, lastSeen: v ? outcomeText(v.outcome) : "None" };
    if (!n.valid) return { ...base, sub: "PoSe banned: a banned masternode can't vote", eligible: false, lastSeen: "—" };
    if (v && !v.shared && v.time !== null && now - v.time < VOTE_CHANGE_SECS) {
      const after = new Date((v.time + VOTE_CHANGE_SECS) * 1000).toLocaleTimeString(locale, { hour: "numeric", minute: "2-digit" });
      return { ...base, sub: `Voted ${outcomeText(v.outcome)} ${ago(v.time, now, locale)} · can change after ${after}`, eligible: false };
    }
    const shared = n.keyHolders > 1 ? ` · ${n.keyHolders} masternodes use this key` : "";
    return { ...base, sub: `key in this wallet · voting key ${n.keyIndex + 1}${shared}`, eligible: true };
  });
}

/** The outcome a sheet starts with: the last vote seen when every eligible row saw the same one
 * (§4.25.4), else none. */
export function presetOutcome(nodes: VotingMasternode[]): VoteOutcomeName | null {
  const seen = new Set(nodes.filter((n) => n.valid).map((n) => seenVote(n)?.outcome ?? null));
  return seen.size === 1 ? [...seen][0] : null;
}

/** "2 masternodes · weight 8 · no fee", for the checked eligible rows. */
export function selectionNote(picked: NodeRowView[]): string {
  const weight = picked.reduce((s, r) => s + r.weight, 0);
  return `${plural(picked.length, "masternode")} · weight ${weight} · no fee`;
}

/** The engine's refusal of a PoSe-banned masternode's vote (`VoteRejection::Banned`, Core's wording). */
export const BANNED_REJECTION = "masternode_not_found";

const REJECTED: Record<string, string> = {
  unknown_masternode: "This masternode isn't in the current masternode list.",
  collateral_unresolved: "Its collateral isn't found yet, so it can't vote from this wallet yet.",
  no_voting_key: "Its voting key is not in this wallet.",
  cancelled: "The wallet locked before this vote was signed.",
  signer_unavailable: "The wallet couldn't read its voting key.",
  in_flight: "The same vote is already being sent.",
  unknown_object: "The network doesn't know this proposal yet.",
  not_a_proposal: "This isn't a proposal.",
  expired: "This proposal has ended.",
  duplicate: "This vote was already sent.",
  malformed: "The vote couldn't be built.",
  bad_signature: "The vote's signature didn't check out.",
  unknown_voting_key: "Its voting key changed, so the key in this wallet no longer matches.",
  wrong_voting_key: "Its voting key changed, so the key in this wallet no longer matches.",
  [BANNED_REJECTION]: "A banned masternode can't vote.",
  obsolete: "A newer vote from this masternode is already known.",
  same_timestamp: "A vote with the same time is already known. Try again in a second.",
  too_often: "The network accepts one change per masternode per hour.",
  too_far_ahead: "This computer's clock is ahead. Check the date and time.",
  key_limit: "This voting key has voted too often on this proposal.",
};

/** A per-masternode or `vote_rejected` reason as a sentence (D13). */
export function rejectedText(reason: string): string {
  return REJECTED[reason] ?? "The network refused the vote.";
}

export interface VoteResultView {
  title: string;
  line: string;
  rows: { proTxHash: string; name: string; sub: string; sent: boolean }[];
}

/** §4.25.4's result: "1 of 2 votes sent", "Yes · weight 4 of 8", a line per masternode. */
export function voteResult(casts: VoteCast[], nodes: VotingMasternode[], outcome: VoteOutcomeName): VoteResultView {
  const byHash = new Map(nodes.map((n) => [n.proTxHash, n]));
  const weightOf = (h: string) => byHash.get(h)?.weight ?? 1;
  const sent = casts.filter((c) => c.rejected === null);
  const total = casts.reduce((s, c) => s + weightOf(c.proTxHash), 0);
  const sentWeight = sent.reduce((s, c) => s + weightOf(c.proTxHash), 0);
  return {
    title: `${sent.length} of ${plural(casts.length, "vote")} sent`,
    line: `${outcomeText(outcome)} · weight ${sentWeight} of ${total}`,
    rows: casts.map((c) => {
      const n = byHash.get(c.proTxHash);
      const name = n ? nodeName(n) : `Masternode ${shortHash(c.proTxHash)}`;
      if (c.rejected !== null) return { proTxHash: c.proTxHash, name, sub: rejectedText(c.rejected), sent: false };
      const peers = c.peers === 0 ? "sent when the wallet reconnects" : `to ${plural(c.peers, "peer")}`;
      return { proTxHash: c.proTxHash, name, sub: `weight ${weightOf(c.proTxHash)} · ${peers}`, sent: true };
    }),
  };
}

// ---------------------------------------------------------------- errors

const REFUSAL: Record<string, string> = {
  public: "This proposal is public already.",
  expired: "This proposal has ended.",
  fee_failed: "Its fee failed, so it can't be submitted.",
  in_flight: "The fee is being sent. Try again in a moment.",
};

/** A governance call's error as a sentence (D13); `null` when it is not a governance code, so
 * the caller can try the send sentences (`screens/pay/model.ts` `failureText`). */
export function governanceErrorText(e: unknown): string | null {
  if (!isAppError(e)) return null;
  switch (e.code) {
    case "governance.sync_disabled":
      return "Governance data is off for this network. Turn it on to continue.";
    case "governance.not_synced":
      return "Proposals are still loading. Try again once they finish.";
    case "governance.proposal_not_found":
      return "The network doesn't know this proposal yet.";
    case "governance.vote_rejected":
      return rejectedText(e.data?.kind === "reason" ? e.data.reason : "");
    case "governance.no_peers":
      return "Not connected to the network.";
    case "governance.list_unavailable":
      return "The masternode list hasn't loaded yet. Try again in a moment.";
    case "governance.invalid_proposal":
      return "Some details of the proposal aren't valid.";
    case "governance.submit_refused":
      return REFUSAL[e.data?.kind === "reason" ? e.data.reason : ""] ?? "This proposal can't be submitted now.";
    case "governance.not_implemented":
      return "This isn't available yet.";
    default:
      return null;
  }
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
    ? "The fee may not have reached the network. Submit sends it again."
    : `The fee may not have reached the network (${why}). Submit sends the same fee again.`;
}

/** The toast once `create_proposal` returned: paid, or kept with its fee unsent (MG-06 D10). */
export function createdToast(p: MyProposal): string {
  return p.state.kind === "feeUnsent" || p.state.kind === "feeFailed"
    ? `${p.name}: created, but its fee didn't go out. Mine shows why.`
    : `${p.name}: fee paid. Mine shows its progress.`;
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
        chip: { text: "Fee not sent", tone: "warning", tooltip: "The fee may not have reached the network." },
        sub: `Created ${created} · its fee hasn't been seen on the network`,
        submit: true,
        error,
        attention: error !== null,
      };
    }
    case "feePaid":
      return {
        ...none,
        chip: { text: "Fee paid", tone: "neutral", tooltip: STATUS_CHIPS.pending.tooltip },
        meta: `${Math.min(s.confirmations, 1)} of 1`,
        sub:
          s.confirmations === 0
            ? `Fee paid ${created} · waiting for its first confirmation, then the wallet submits it`
            : `Fee paid ${created} · confirmed; the wallet is submitting it`,
        submit: s.confirmations === 0,
        error: s.confirmations === 0 ? lastErrorText(p) : null,
      };
    case "readyToSubmit":
      if (s.reason === "noPeerAccepted") {
        return {
          ...none,
          chip: { text: "Ready to submit", tone: "warning", tooltip: "Its fee is confirmed, but no peer has taken the proposal yet." },
          sub: `Fee paid ${created}`,
          submit: true,
          error: "No peer accepted the proposal. Try again in a few minutes.",
          attention: true,
        };
      }
      return {
        ...none,
        chip: { text: "Ready to submit", tone: "warning", tooltip: "Its fee is confirmed; the wallet submits it once governance data is loaded." },
        sub: s.reason === "syncDisabled" ? "Governance data is off. Turn it on to submit." : "Waiting for governance data to load, then the wallet submits it",
        turnOn: s.reason === "syncDisabled",
      };
    case "confirming":
      return {
        ...none,
        chip: STATUS_CHIPS.confirming,
        meta: `${Math.min(s.confirmations, FULL_CONFIRMATIONS)} of ${FULL_CONFIRMATIONS}`,
        sub: "Submitted after the first confirmation · fully valid after 6",
      };
    case "public":
      return {
        ...none,
        chip: s.status ? statusChip(s.status) : { text: "Submitted", tone: "neutral", tooltip: "Public; its votes show once proposals are loaded." },
        sub: p.submittedAt !== null ? `Submitted ${dateTime(p.submittedAt, locale)}` : `Created ${created}`,
      };
    case "feeFailed":
      return {
        ...none,
        chip: { text: "Fee failed", tone: "danger", tooltip: "The fee transaction was rejected or replaced." },
        sub: "The fee transaction was rejected or replaced, so this proposal can't be submitted.",
      };
    case "expired":
      return { ...none, chip: STATUS_CHIPS.lapsed, sub: `Created ${created}` };
  }
}

/** The sidebar badge's governance part (MG-00 D8, D5). */
export function badgeCount(mine: MyProposal[] | undefined, locale?: string): number {
  return (mine ?? []).filter((p) => mineRow(p, locale).attention).length;
}

/** "A proposal in progress" for the sidebar (MG-00 D1, D3). */
export const inProgress = (p: MyProposal) => p.state.kind !== "expired" && p.state.kind !== "feeFailed";

/** A masternode or evonode collateral's amount, in duffs (MG-00 D1 (2), D3). */
const COLLATERALS = [1_000 * 100_000_000, 4_000 * 100_000_000];

/** The sidebar item's visibility (D3): the setting, a proposal in progress, or a coin shaped
 * like a collateral. */
export function masternodesShown(setting: boolean, mine: MyProposal[] | undefined, coinAmounts: number[] | undefined): boolean {
  return setting || (mine ?? []).some(inProgress) || (coinAmounts ?? []).some((a) => COLLATERALS.includes(a));
}

// ---------------------------------------------------------------- list filters

export type ListFilter = "all" | "needsVote" | "mine";
export type SortOrder = "closest" | "mostAsked" | "newest" | "name";
export const SORTS: { id: SortOrder; title: string }[] = [
  { id: "closest", title: "Closest to passing" },
  { id: "mostAsked", title: "Most asked" },
  { id: "newest", title: "Newest" },
  { id: "name", title: "Name" },
];

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
      return "Use 1 to 40 lower case letters, digits, - and _.";
    case "url":
      return "Enter the page's address, starting with https://.";
    case "payment_address":
      return `Enter a Dash address on ${chainName}.`;
    case "payment_amount":
      return "Enter an amount the budget can pay at one superblock.";
    case "payment_count":
      return `Choose 1 to ${MAX_PAYMENTS} payments.`;
    case "first_payment":
      return "Choose one of the next superblocks.";
    case "payload":
      return "The name and link are too long together. Shorten them.";
  }
}

/** The local check of step 1, before the engine's (`preview_proposal`). */
export function detailsProblems(name: string, url: string): Partial<Record<"name" | "url", string>> {
  const out: Partial<Record<"name" | "url", string>> = {};
  if (!NAME_PATTERN.test(name)) out.name = fieldText("name", "");
  if (url.trim() === "") out.url = "Enter a link to a page describing the proposal.";
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
    notice: `The estimated cutoff for the ${shortDate(first.estimatedTime, locale)} payout is in about ${left}, so the first payment is ${shortDate(second.estimatedTime, locale)}. New proposals rarely gather enough votes in a day.`,
  };
}

/** "Dec 4, 2026 (block 2,391,664)". */
export function superblockOption(sb: SuperblockDate, locale?: string): string {
  return `${longDate(sb.estimatedTime, locale)} (block ${fmt(sb.height)})`;
}

/** "Last payment Feb 1, 2027." from the chosen superblock and count. */
export function lastPaymentText(superblocks: SuperblockDate[], first: number | null, count: number, locale?: string): string | null {
  const i = superblocks.findIndex((s) => s.height === first);
  if (i < 0) return null;
  if (count === 1) return "One payment.";
  const last = superblocks[i + count - 1];
  if (last) return `Last payment ${longDate(last.estimatedTime, locale)}.`;
  // Past the list: one cycle per payment from the last one known.
  const known = superblocks[superblocks.length - 1];
  const cycle = superblocks.length > 1 ? superblocks[1].estimatedTime - superblocks[0].estimatedTime : 0;
  return `Last payment about ${longDate(known.estimatedTime + cycle * (i + count - superblocks.length), locale)}.`;
}

/** The budget notice of step 2, shown once proposals are synced (§4.25.3, MG-00 D45). */
export function budgetNotice(info: GovernanceInfo | undefined, amount: (duffs: number) => string): string | null {
  if (!info || info.allocatedEstimate === null) return null;
  return `Up to ${amount(info.budget)} is available at each superblock. From the votes peers have sent, passing proposals already ask for about ${amount(info.allocatedEstimate)}, so yours fits only if it passes with more votes than others that don't fit.`;
}

// ---------------------------------------------------------------- Governance numbers

/** The rows of Governance numbers… (MG-00 D20, §4.25.3). */
export function numbersRows(info: GovernanceInfo, sync: GovernanceSyncState | undefined, rows: ProposalRow[] | undefined, amount: (d: number) => string, locale?: string) {
  const c = info.clock;
  const regular = info.validMasternodes - info.validEvonodes;
  const cycle = [
    { label: "Last superblock", value: c.lastSuperblock > 0 ? `Block ${fmt(c.lastSuperblock)}` : "None yet" },
    {
      label: "Estimated cutoff",
      value: `Block ${fmt(c.votingCutoffHeight)} · about ${shortDate(c.votingCutoffEstimate, locale)}: votes after it may be too late for the ${shortDate(c.superblockEta, locale)} payout`,
    },
    { label: "Next superblock", value: `Block ${fmt(c.nextSuperblock)} · about ${shortDate(c.superblockEta, locale)}` },
    { label: "Cycle", value: `${fmt(info.superblockCycle)} blocks; the network starts fixing a payout ${fmt(info.maturityWindow)} blocks before its superblock` },
  ];
  const synced = info.proposals !== null && info.allocatedEstimate !== null;
  const open = rows?.filter((r) => !isEnded(r)).length;
  const votes = [
    { label: "Voting weight", value: `${fmt(info.validWeight)}: ${fmt(regular)} masternodes and ${fmt(info.validEvonodes)} evonodes, which count 4 each` },
    { label: "Funding line", value: `${fmt(info.passingThreshold)} net yes votes: a tenth of the weight, and at least the minimum quorum` },
    { label: "Budget", value: `${amount(info.budget)} per superblock` },
    {
      label: "Passing, estimated",
      value:
        synced && info.proposals
          ? `about ${amount(info.allocatedEstimate!)} to about ${fmt(info.proposals.passing)} of ${fmt(open ?? info.proposals.total)} open · about ${fmt(info.proposals.unfunded)} more over budget`
          : "after sync",
    },
    {
      label: "Data",
      value: synced ? `Block ${fmt(c.tipHeight)} · ${plural(sync?.peers ?? 0, "peer")}; some votes may be missing` : "after sync",
    },
  ];
  return { cycle, votes };
}
