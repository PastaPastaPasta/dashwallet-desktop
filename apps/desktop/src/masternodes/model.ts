// My masternodes' view model (MG-10 §1.2, §1.7, §1.9, §3.8): row names, chips, groups, the
// summary, the pane's role and payout lines, credits, and the errors as sentences.

import { isAppError } from "../api/backend";
import type { AttachedKeyRoleName } from "../api/generated/AttachedKeyRoleName";
import type { KeyCheckName } from "../api/generated/KeyCheckName";
import type { LockStateName } from "../api/generated/LockStateName";
import type { MasternodeDetail } from "../api/generated/MasternodeDetail";
import type { MasternodeOverview } from "../api/generated/MasternodeOverview";
import type { MasternodePayout } from "../api/generated/MasternodePayout";
import type { MasternodeRow } from "../api/generated/MasternodeRow";
import type { MasternodeStatusName } from "../api/generated/MasternodeStatusName";
import type { MasternodeTypeName } from "../api/generated/MasternodeTypeName";
import { creditsToDuffs } from "../api/pending";
import { withTable } from "../dashpay/status";
import { shortDate, shortHash, type ChipView, type NodeNames } from "../governance/model";
import { messageTable, t } from "../i18n";

const P = "masternodes.myMasternodes";

export const MnText = withTable(
  messageTable(P, [
    "tab",
    "loading",
    "listSyncing",
    "failed",
    "payoutsFailed",
    "tryAgain",
    "emptyTitle",
    "emptyText",
    "watchOnly",
    "watchOnlyText",
    "unmatched",
    "incomplete",
    "seeInActivity",
    "banned",
    "vote",
    "voteBanned",
    "voteNoKey",
    "voteRetired",
    "voteRegistering",
    "rename",
    "renameTitle",
    "copyHash",
    "copyOutpoint",
    "unlock",
    "poseScore",
    "sharedPayout",
    "flaggedHeading",
    "payoutsHeading",
    "payoutProvenance",
  ]),
  {
    listHeight: (h: number) => t(`${P}.listHeight`, { height: h }),
    get emptyCoin() {
      return t(`${P}.emptyCoin`, { amount: 1000 });
    },
    get emptyPoints() {
      return (["own", "runs", "earn"] as const).map((k) => [t(`${P}.emptyPoints.${k}.title`), t(`${P}.emptyPoints.${k}.text`, { amount: 1000, evonode: 4000 })] as const);
    },

    bannedText: (seen: string | null) => (seen ? t(`${P}.bannedTextSeen`, { date: seen }) : t(`${P}.bannedText`)),
  },
);

/** A row whose type the list hasn't given yet reads "Masternode". */
export const kindText = (k: MasternodeTypeName | null) => t(k === "evonode" ? `${P}.kind.evonode` : `${P}.kind.masternode`);

/** "Home node", else "Masternode 2" / "Evonode 1" by its type index; a tracked one (index 0) by its hash. */
export const rowName = (r: Pick<MasternodeRow, "label" | "kind" | "typeIndex" | "proTxHash">) =>
  r.label ?? t(`${P}.rowName`, { kind: kindText(r.kind), index: r.typeIndex > 0 ? String(r.typeIndex) : shortHash(r.proTxHash) });

const TONE: Record<MasternodeStatusName, ChipView["tone"]> = { enabled: "success", pose_banned: "danger", removed: "neutral", unknown: "neutral", unconfirmed: "info" };
const CHIPS = Object.fromEntries(
  (Object.keys(TONE) as MasternodeStatusName[]).map((s) => [
    s,
    {
      get text() {
        return t(`${P}.chip.${s}`);
      },
      tone: TONE[s],
      tooltip: "",
    },
  ]),
) as Record<MasternodeStatusName, ChipView>;
export const statusChip = (s: MasternodeStatusName) => CHIPS[s];

/** Banned, with this wallet's owner key or collateral: the Needs attention group and the badge. */
export const needsAttention = (r: MasternodeRow) => r.status === "pose_banned" && r.mine && (r.roles.owner || r.roles.collateral);

export type GroupId = "attention" | "mine" | "tracked" | "retired";
export const GROUP_TITLE: Readonly<Record<GroupId, string>> = messageTable(`${P}.group`, ["attention", "mine", "tracked", "retired"]);

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
export const rowSub = (r: MasternodeRow) => t(`${P}.rowSub`, { kind: kindText(r.kind), service: r.service ?? t(`${P}.notInList`) });

/** A paid date only from an attributed record (§1.2: `payouts_incomplete` covers 30 days only). */
export function rowMeta(r: MasternodeRow, synced: boolean, locale?: string): string | null {
  if (needsAttention(r) && r.bannedSeenAt !== null) return t(`${P}.noticed`, { date: shortDate(r.bannedSeenAt, locale) });
  if (!synced && r.mine) return MnText.listSyncing;
  if (r.lastPayout && !r.lastPayout.unattributed && r.lastPayout.timestamp !== null) return t(`${P}.paid`, { date: shortDate(r.lastPayout.timestamp, locale) });
  return r.tracked && !r.mine ? t(`${P}.tracked`) : null;
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
  const unmatched = o.unattributed30d > 0;
  return {
    count: mine.length,
    countNote: tracked > 0 ? t(`${P}.summary.activeTracked`, { active, tracked }) : t(`${P}.summary.active`, { count: active }),
    payouts: o.payouts30d,
    unattributed: unmatched ? o.unattributed30d : null,
    unattributedNote: unmatched ? (o.payoutsIncomplete ? t(`${P}.unmatchedIncomplete`) : MnText.unmatched) : null,
  };
}

export interface Held {
  id: string;
  text: string;
  held: boolean;
}

/** "In this wallet": what the wallet holds, then what it doesn't; a watch-only wallet's are "(watched)". */
export function heldRoles(r: MasternodeRow, watchOnly: boolean): Held[] {
  const all: Held[] = [
    { id: "owner", text: t(`${P}.role.owner`), held: r.roles.owner },
    { id: "voting", text: t(`${P}.role.voting`), held: r.roles.voting },
    { id: "payout", text: t(`${P}.role.payout`), held: r.roles.payout },
    { id: "collateral", text: t(`${P}.role.collateral`, { amount: r.kind === "evonode" ? 4000 : 1000 }), held: r.roles.collateral },
    { id: "operator", text: t(`${P}.role.operator`), held: r.roles.operator },
  ];
  const held = all.filter((h) => h.held).map((h) => (watchOnly ? { ...h, text: t(`${P}.watched`, { role: h.text }) } : h));
  return [...held, ...all.filter((h) => !h.held)];
}

/** Why Vote is off, or `null` when it's on. */
export function voteBlocked(r: MasternodeRow, watchOnly: boolean): string | null {
  if (watchOnly) return t(`${P}.voteWatchOnly`);
  if (r.status === "pose_banned") return MnText.voteBanned;
  if (r.status === "removed") return MnText.voteRetired;
  if (r.status === "unconfirmed") return MnText.voteRegistering;
  return r.roles.voting ? null : MnText.voteNoKey;
}

// ---------------------------------------------------------------- track and keys (§1.3, §1.4)

const K = `${P}.keys`;

/** "Voting key" for the offer, the name "In this wallet" uses. */
const roleTitle = (role: AttachedKeyRoleName) => t(`${P}.role.${role}`);
/** "voting", "owner", "payout": a key's role mid-sentence. */
const roleWord = (role: AttachedKeyRoleName) => t(`${K}.roleWord.${role}`);

export interface KeyRoleView {
  role: AttachedKeyRoleName;
  title: string;
  help: string;
}

/** The keys Add keys offers: the voting key only (MG-00 D13, amended for 1.0 by MG's ruling of 11-24), and not
 * while one is attached: Add key never replaces a key; the pane's Remove comes first. Owner, operator and
 * payout keys are never imported here. While the vault is locked the attached keys are unread: the sheet
 * asks for the unlock first. */
export function keyRoles(r: Pick<MasternodeRow, "roles">): KeyRoleView[] {
  // Unread (locked): nothing is offered until the attached keys are known.
  if (r.roles.attached === null || r.roles.attached.some((k) => k.role === "voting")) return [];
  return [{ role: "voting", title: roleTitle("voting"), help: t(`${K}.votingHelp`) }];
}

/** More ▸ Add keys… on a tracked row: while a key can be added, or the vault must be unlocked to tell. */
export const offersAddKey = (r: MasternodeRow) => r.tracked && (r.roles.attached === null || keyRoles(r).length > 0);

/** Stop tracking's line about the added keys: their count, or, while they're unread, that any are deleted. */
export function untrackKeysText(r: Pick<MasternodeRow, "roles">): string | null {
  if (r.roles.attached === null) return t(`${P}.untrack.keysUnknown`);
  return r.roles.attached.length > 0 ? t(`${P}.untrack.keys`, { count: r.roles.attached.length }) : null;
}

/** Why an Add key sheet the vault could write shows no field: the added keys aren't read yet, or a voting key
 * was added meanwhile and must be removed first. */
export function addKeyWaitText(r: Pick<MasternodeRow, "roles">): string | null {
  if (r.roles.attached === null) return t("masternodes.keys.loading");
  return keyRoles(r).length === 0 ? t(`${K}.exists`, { role: roleWord("voting") }) : null;
}

/** The vault holds its full-scope key: an attach can be written (tracking.rs refuses `locked` and mixing-only). */
export const canAttach = (state: LockStateName | undefined) => state !== "locked" && state !== "unlockedMixingOnly";

/** What `mn_attach_key` answered for a voting key, as §1.4's result line. */
export function keyResultText(check: KeyCheckName): string {
  if (check === "matched") return t(`${K}.matched`);
  if (check === "key_mismatch") return t(`${K}.mismatch`);
  return t(`${K}.unchecked`);
}

/** An attach refusal as a sentence: a key that isn't this masternode's, a malformed key, a locked vault. */
export function keyErrorText(e: unknown): string {
  if (isAppError(e)) {
    if (e.code === "masternodes.key_mismatch") return t(`${K}.mismatch`);
    if (e.code === "masternodes.key_exists") {
      const role = e.data?.kind === "masternodeKey" ? e.data.role : "voting";
      return t(`${K}.exists`, { role: roleWord(role) });
    }
    if (e.code === "invalid_argument") return t(`${K}.invalid`);
    if (e.code === "vault.locked") return t(`${K}.locked`);
  }
  return mnErrorText(e);
}

export const isVaultLocked = (e: unknown) => isAppError(e) && e.code === "vault.locked";

export interface AttachedKeyLine {
  role: AttachedKeyRoleName;
  text: string;
  /** "Remove payout key": it deletes a private key, so it says so. */
  remove: string;
  /** The confirmation every Remove asks first. */
  confirmTitle: string;
}

/** The pane's "Keys you added", every role (voting included); `null` while the vault is locked. */
export function attachedLines(r: MasternodeRow): AttachedKeyLine[] | null {
  if (r.roles.attached === null) return null;
  return r.roles.attached.map((k) => ({
    role: k.role,
    text: t(`${K}.line`, { role: t(`${K}.name.${k.role}`), check: t(`${K}.check.${k.check}`) }),
    remove: t(`${K}.removeRow`, { role: roleWord(k.role) }),
    confirmTitle: t(`${K}.removeTitle`, { role: roleWord(k.role) }),
  }));
}

/** Tickets for a search whose answers may arrive out of order: only the latest started, not dropped since,
 * is current, so an answer to an earlier query never shows (or offers Track) under a later one. */
export function latestRequest() {
  let n = 0;
  return { start: () => ++n, isCurrent: (ticket: number) => ticket === n, drop: () => void n++ };
}

/** A Track refusal as §1.3's sentence; `name` is the already-tracked masternode's. */
export function trackErrorText(e: unknown, name: string | null): string {
  if (isAppError(e)) {
    if (e.code === "invalid_argument") return t(`${P}.track.invalid`);
    if (e.code === "masternodes.list_unavailable") return t(`${P}.track.list_unavailable`);
    if (e.code === "masternodes.already_tracked" && name) return t(`${P}.track.already`, { name });
  }
  return mnErrorText(e);
}

/** §1.7's line in place of Change keys or payout, which shows only while the registrar is `available`. */
export function registrarLine(d: MasternodeDetail): { text: string; unlock: boolean } | null {
  switch (d.registrar.kind) {
    case "registration_unavailable": {
      const withdraw = d.row.kind === "evonode" && (d.row.roles.attached ?? []).some((k) => (k.role === "owner" || k.role === "payout") && k.check === "matched");
      return { text: t(`${P}.registrar.${withdraw ? "registrationUnavailableWithdraw" : "registration_unavailable"}`), unlock: false };
    }
    case "no_owner_key":
      return { text: t(`${P}.registrar.${d.registrar.kind}`), unlock: false };
    case "vault_locked":
      return { text: t(`${P}.registrar.vault_locked`), unlock: true };
    default:
      return null;
  }
}

/** Operator reward in hundredths of a percent, as Core stores it. */
export const operatorShare = (reward: number) => t(`${P}.fact.operatorShareValue`, { share: reward / 100 });

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
export const lastPayoutTitle = (p: MasternodePayout) => t(p.unattributed ? `${P}.fact.lastPayment` : `${P}.fact.lastPayout`);

/** Another of the wallet's rows has the same payout address. */
export const sharesPayoutAddress = (r: MasternodeRow, rows: MasternodeRow[]) =>
  r.payoutAddress !== null && rows.some((o) => o.proTxHash !== r.proTxHash && o.payoutAddress === r.payoutAddress);

/** Activity's chip over a node's txids (§1.9). */
export const activityChip = (name: string, flagged: boolean) => t(flagged ? `${P}.activity.flagged` : `${P}.activity.payouts`, { name });

/** Credits (10^11 per Dash) through the Dash formatter, never a raw integer (§1). */
export function formatCredits(credits: string, fmt: (duffs: number) => string): string {
  return fmt(creditsToDuffs(credits));
}

/** Labels and services by ProTx hash, for the governance vote rows (MG-09 F3). */
export const nodeNames = (o: MasternodeOverview | undefined): NodeNames => new Map((o?.masternodes ?? []).map((r) => [r.proTxHash, { label: rowName(r), service: r.service }]));

const ERROR_KEYS = [
  "list_unavailable",
  "not_found",
  "not_evonode",
  "no_identity",
  "platform_unavailable",
  "unsupported",
  "in_flight",
  "operation_state",
  "update_pending",
  "update_superseded",
  "share_not_mine",
  "shared_too_early",
  "shared_registration_unknown",
] as const;
const ERRORS = messageTable(`${P}.errors`, ERROR_KEYS);

/** A masternodes call's error as a sentence (§3.8); unknown codes keep the engine's message. */
export function mnErrorText(e: unknown): string {
  if (!isAppError(e)) return e instanceof Error ? e.message : String(e);
  if (e.code === "masternodes.no_owner_key") return t(`${P}.errors.no_owner_key`);
  if (e.code === "vault.locked") return t(`${P}.errors.vault_locked`);
  const key = e.code.startsWith("masternodes.") ? e.code.slice("masternodes.".length) : null;
  return key !== null && Object.hasOwn(ERRORS, key) ? ERRORS[key as (typeof ERROR_KEYS)[number]] : e.message;
}
