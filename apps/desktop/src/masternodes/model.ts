// My masternodes' view model (MG-10 §1.2, §1.7, §1.9, §3.8): row names, chips, groups, the
// summary, the pane's role and payout lines, credits, and the errors as sentences.

import { isAppError } from "../api/backend";
import type { MasternodeDetail } from "../api/generated/MasternodeDetail";
import type { MasternodeOverview } from "../api/generated/MasternodeOverview";
import type { MasternodePayout } from "../api/generated/MasternodePayout";
import type { MasternodeRow } from "../api/generated/MasternodeRow";
import type { MasternodeStatusName } from "../api/generated/MasternodeStatusName";
import type { MasternodeTypeName } from "../api/generated/MasternodeTypeName";
import { creditsToDuffs } from "../api/pending";
import { shortDate, shortHash, type ChipView, type NodeNames } from "../governance/model";

export const MnText = {
  tab: "My masternodes",
  listHeight: (h: number) => `Masternode list at block ${h.toLocaleString("en-US")}`,
  loading: "Loading the masternode list…",
  listSyncing: "list syncing",
  failed: "Couldn't read your masternodes.",
  payoutsFailed: "Couldn't read this masternode's payouts.",
  tryAgain: "Try again",
  emptyTitle: "No masternodes yet",
  emptyText:
    "Masternodes and evonodes registered with this wallet's keys appear here after the wallet syncs. You can also track any masternode on the network by its IP address or ProTx hash.",
  emptyCoin: "This wallet holds a 1,000 Dash output that can be a masternode collateral.",
  emptyPoints: [
    ["Own one", "Lock 1,000 Dash (4,000 for an evonode) as collateral. It stays in your wallet."],
    ["Someone runs it", "A hosting provider, or you, runs the server. The wallet never needs the server's key."],
    ["Earn and vote", "Payouts arrive in this wallet, and the node votes on how the network spends its budget."],
  ] as const,
  watchOnly: "Watch-only wallet.",
  watchOnlyText: "It shows the masternodes whose collateral or payout address it watches, and their payouts. Voting, registering and changing keys need the wallet that holds the keys.",
  unmatched: "The wallet can't match these payments to a masternode.",
  incomplete: "This network has shared masternodes, whose payouts it can't tell apart yet.",
  seeInActivity: "See in Activity",
  banned: "This masternode is banned and earns nothing.",
  bannedText: (seen: string | null) =>
    `${seen ? `This wallet first saw it banned on ${seen}: the` : "The"} server stopped doing its work. Your operator fixes the server, then sends a service update from the node. The wallet can't do that: it needs the operator key.`,
  vote: "Vote on proposals",
  voteBanned: "A banned masternode can't vote",
  voteNoKey: "Its voting key is not in this wallet",
  voteRetired: "A retired masternode can't vote",
  voteRegistering: "A masternode can vote once its registration confirms",
  rename: "Rename…",
  renameTitle: "Rename masternode",
  copyHash: "Copy ProTx hash",
  copyOutpoint: "Copy collateral outpoint",
  unlock: "Unlock",
  poseScore: "(needs a full node)",
  sharedPayout: "Matched by payout address: masternodes that share it show the same payments.",
  flaggedHeading: "Payments to this payout address (may include shared-masternode payouts)",
  payoutsHeading: "Payouts, last 30 days",
  payoutProvenance: "from this wallet's registration records; a change made elsewhere wouldn't show here",
} as const;

/** A row whose type the list hasn't given yet reads "Masternode". */
export const kindText = (k: MasternodeTypeName | null) => (k === "evonode" ? "Evonode" : "Masternode");

/** "Home node", else "Masternode 2" / "Evonode 1" by its type index; a tracked one (index 0) by its hash. */
export const rowName = (r: Pick<MasternodeRow, "label" | "kind" | "typeIndex" | "proTxHash">) =>
  r.label ?? `${kindText(r.kind)} ${r.typeIndex > 0 ? r.typeIndex : shortHash(r.proTxHash)}`;

const CHIPS: Record<MasternodeStatusName, ChipView> = {
  enabled: { text: "Active", tone: "success", tooltip: "" },
  pose_banned: { text: "PoSe banned", tone: "danger", tooltip: "" },
  removed: { text: "Retired", tone: "neutral", tooltip: "" },
  unknown: { text: "Unknown", tone: "neutral", tooltip: "" },
  unconfirmed: { text: "Registering", tone: "info", tooltip: "" },
};
export const statusChip = (s: MasternodeStatusName) => CHIPS[s];

/** Banned, with this wallet's owner key or collateral: the Needs attention group and the badge. */
export const needsAttention = (r: MasternodeRow) => r.status === "pose_banned" && r.mine && (r.roles.owner || r.roles.collateral);

export type GroupId = "attention" | "mine" | "tracked" | "retired";
export const GROUP_TITLE: Record<GroupId, string> = { attention: "Needs attention", mine: "My masternodes", tracked: "Tracked", retired: "Retired" };

export function groupOf(r: MasternodeRow): GroupId | null {
  if (r.status === "removed") return "retired";
  if (needsAttention(r)) return "attention";
  if (r.mine) return "mine";
  return r.tracked ? "tracked" : null;
}

export function groups(rows: MasternodeRow[]): { id: GroupId; rows: MasternodeRow[] }[] {
  const order: GroupId[] = ["attention", "mine", "tracked", "retired"];
  return order.map((id) => ({ id, rows: rows.filter((r) => groupOf(r) === id) })).filter((g) => g.rows.length > 0);
}

/** The sidebar badge's S1 half (§3.6). */
export const bannedCount = (o: MasternodeOverview | undefined) => o?.masternodes.filter(needsAttention).length ?? 0;

export const isEmpty = (o: MasternodeOverview) => o.masternodes.length === 0;

/** "Masternode · 203.0.113.7:19999". */
export const rowSub = (r: MasternodeRow) => `${kindText(r.kind)} · ${r.service ?? "not in the list"}`;

/** A paid date only from an attributed record (§1.2: `payouts_incomplete` covers 30 days only). */
export function rowMeta(r: MasternodeRow, synced: boolean, locale?: string): string | null {
  if (needsAttention(r) && r.bannedSeenAt !== null) return `noticed ${shortDate(r.bannedSeenAt, locale)}`;
  if (!synced && r.mine) return MnText.listSyncing;
  if (r.lastPayout && !r.lastPayout.unattributed && r.lastPayout.timestamp !== null) return `Paid ${shortDate(r.lastPayout.timestamp, locale)}`;
  return r.tracked && !r.mine ? "Tracked" : null;
}

export interface SummaryView {
  count: number;
  countNote: string;
  payouts: number;
  unattributed: number | null;
  unattributedNote: string | null;
}

export function summary(o: MasternodeOverview): SummaryView {
  const live = o.masternodes.filter((r) => r.status !== "removed");
  const mine = live.filter((r) => r.mine);
  const active = mine.filter((r) => r.status === "enabled").length;
  const tracked = live.filter((r) => r.tracked && !r.mine).length;
  const parts = [`${active} active`, ...(tracked > 0 ? [`${tracked} tracked`] : [])];
  const unmatched = o.unattributed30d > 0;
  return {
    count: mine.length,
    countNote: parts.join(" · "),
    payouts: o.payouts30d,
    unattributed: unmatched ? o.unattributed30d : null,
    unattributedNote: unmatched ? [MnText.unmatched, ...(o.payoutsIncomplete ? [MnText.incomplete] : [])].join(" ") : null,
  };
}

export interface Held {
  id: string;
  text: string;
  held: boolean;
}

/** "In this wallet": what the wallet holds, then what it doesn't; a watch-only wallet's are "(watched)". */
export function heldRoles(r: MasternodeRow, watchOnly: boolean): Held[] {
  const w = watchOnly ? " (watched)" : "";
  const all: Held[] = [
    { id: "owner", text: "Owner key", held: r.roles.owner },
    { id: "voting", text: "Voting key", held: r.roles.voting },
    { id: "payout", text: "Payout address", held: r.roles.payout },
    { id: "collateral", text: `Collateral, ${r.kind === "evonode" ? "4,000" : "1,000"} Dash`, held: r.roles.collateral },
    { id: "operator", text: "Operator key", held: r.roles.operator },
  ];
  return [...all.filter((h) => h.held).map((h) => ({ ...h, text: `${h.text}${w}` })), ...all.filter((h) => !h.held)];
}

/** Why Vote is off, or `null` when it's on. */
export function voteBlocked(r: MasternodeRow, watchOnly: boolean): string | null {
  if (watchOnly) return "Voting needs the wallet that holds the voting keys";
  if (r.status === "pose_banned") return MnText.voteBanned;
  if (r.status === "removed") return MnText.voteRetired;
  if (r.status === "unconfirmed") return MnText.voteRegistering;
  return r.roles.voting ? null : MnText.voteNoKey;
}

/** §1.7's line in place of Change keys (S1 shows the line only, never the action). */
export function registrarLine(d: MasternodeDetail): { text: string; unlock: boolean } | null {
  switch (d.registrar.kind) {
    case "registration_unavailable":
      return {
        text: "Changing keys needs this masternode's registration, which this wallet doesn't hold. You can still withdraw evonode credits with an added owner or payout key.",
        unlock: false,
      };
    case "no_owner_key":
      return { text: "This wallet doesn't hold this masternode's owner key.", unlock: false };
    case "vault_locked":
      return { text: "Unlock to check the keys you added.", unlock: true };
    default:
      return null;
  }
}

/** Operator reward in hundredths of a percent, as Core stores it. */
export const operatorShare = (reward: number) => `${reward / 100}% of each payout`;

export interface PayoutGroup {
  heading: string;
  total: number;
  count: number;
  txids: string[];
  flagged: boolean;
}

/** The pane's 30 days (§1.2, Q2): attributed payouts, and flagged ones under their own heading. */
export function payoutGroups(records: MasternodePayout[]): PayoutGroup[] {
  const of = (flagged: boolean): PayoutGroup => {
    const rs = records.filter((p) => p.unattributed === flagged);
    return { heading: flagged ? MnText.flaggedHeading : MnText.payoutsHeading, total: rs.reduce((s, p) => s + p.amount, 0), count: rs.length, txids: [...new Set(rs.map((p) => p.txid))], flagged };
  };
  return [of(false), of(true)].filter((g) => g.count > 0);
}

/** "Last payout" or, for a record the wallet can't attribute, "Last payment to its payout address". */
export const lastPayoutTitle = (p: MasternodePayout) => (p.unattributed ? "Last payment to its payout address" : "Last payout");

/** Another of the wallet's rows has the same payout address. */
export const sharesPayoutAddress = (r: MasternodeRow, rows: MasternodeRow[]) =>
  r.payoutAddress !== null && rows.some((o) => o.proTxHash !== r.proTxHash && o.payoutAddress === r.payoutAddress);

/** Activity's chip over a node's txids (§1.9). */
export const activityChip = (name: string, flagged: boolean) => (flagged ? `Payments to ${name}'s payout address` : `Payouts of ${name}`);

/** Credits (10^11 per Dash) through the Dash formatter, never a raw integer (§1). */
export function formatCredits(credits: string, fmt: (duffs: number) => string): string {
  return fmt(creditsToDuffs(credits));
}

/** Labels and services by ProTx hash, for the governance vote rows (MG-09 F3). */
export const nodeNames = (o: MasternodeOverview | undefined): NodeNames => new Map((o?.masternodes ?? []).map((r) => [r.proTxHash, { label: rowName(r), service: r.service }]));

const ERRORS: Record<string, string> = {
  "masternodes.list_unavailable": "The masternode list isn't loaded yet. Try again once the wallet has synced.",
  "masternodes.not_found": "This masternode isn't in the masternode list.",
  "masternodes.not_evonode": "This masternode isn't an evonode.",
  "masternodes.no_identity": "Platform has no identity for this evonode yet.",
  "masternodes.platform_unavailable": "Platform isn't answering. Try again later.",
  "masternodes.unsupported": "This wallet can't read this yet.",
  "masternodes.in_flight": "Another step on this masternode is still running.",
  "masternodes.operation_state": "This item changed since you opened it.",
  "masternodes.update_pending":
    "An earlier change to this masternode hasn't reached the masternode list yet. Wait for it, or discard it in In progress if it was never sent.",
  "masternodes.update_superseded": "A newer change to this masternode replaced this one.",
  "masternodes.share_not_mine": "This share isn't held by this wallet.",
  "masternodes.shared_too_early": "Available once the masternode list includes the registration.",
  "masternodes.shared_registration_unknown": "This wallet's history doesn't hold this masternode's registration.",
  "vault.locked": "Unlock the wallet first.",
};

/** A masternodes call's error as a sentence (§3.8); unknown codes keep the engine's message. */
export function mnErrorText(e: unknown): string {
  if (!isAppError(e)) return e instanceof Error ? e.message : String(e);
  if (e.code === "masternodes.no_owner_key") return "This wallet doesn't hold this masternode's owner key.";
  return ERRORS[e.code] ?? e.message;
}
