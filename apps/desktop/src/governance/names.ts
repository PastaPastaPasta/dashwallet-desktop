// Masternodes ▸ Username voting's view model (MG-11 over MG-07's `contested_names` and
// `vote_name`): pure functions from the records to what the tab, the pane, the vote sheet and its
// result show. The D-rows cited are docs/design/MG-11-contested-voting-ui.md's unless they say
// "MG-07" or "MG-09".

import { isAppError } from "../api/backend";
import type { ContestedName } from "../api/generated/ContestedName";
import type { ContestedNames } from "../api/generated/ContestedNames";
import type { NameChoice } from "../api/generated/NameChoice";
import type { NameRejectionName } from "../api/generated/NameRejectionName";
import type { NameVoteCast } from "../api/generated/NameVoteCast";
import type { NameVoter } from "../api/generated/NameVoter";
import type { NameVoterState } from "../api/generated/NameVoterState";
import { withTable } from "../dashpay/status";
import { messageTable, t } from "../i18n";
import { dateTime, governanceErrorText, nodeName, shortDate, shortHash, spanLong, type ChipView, type NodeNames } from "./model";

const P = "masternodes.names";

/** A contest ending within this reads "Ends in …" on its row (D7). */
const ENDING_SOON_SECS = 86_400;

export const NameText = withTable(
  messageTable(P, [
    "tab", "intro", "search", "browsingOnly", "browsingOnlyText", "watchOnly", "watchOnlyText",
    "voteWatchOnly", "noVoters", "empty", "emptyHelp", "loading", "failed", "stale", "ended", "endedSentence",
    "tryAgain", "privacy", "partial", "normalized", "counts",
  ]),
  { limitNote: (allowed: number) => t(`${P}.limitNote`, { count: allowed, changes: allowed - 1 }) },
);

// ---------------------------------------------------------------- names and choices

/** "8ZtW…R97P": an identity id (base58), shortened (D5). */
export const shortIdentity = (id: string) => (id.length > 12 ? `${id.slice(0, 4)}…${id.slice(-4)}` : id);

/** A contest as far as naming its contenders goes. */
export type NameContest = Pick<ContestedName, "label" | "contenders">;

/** "Alice (8ZtW…R97P)" for a contender whose own spelling of the name Platform sent, else
 * "8ZtW…R97P" (D5, F1). */
export const contenderText = (identity: string, name?: string | null): string =>
  name ? t(`${P}.contender`, { name, identity: shortIdentity(identity) }) : shortIdentity(identity);

/** The spelling `contest` lists for `identity`, if any. */
const nameIn = (identity: string, contest?: Pick<ContestedName, "contenders">) => contest?.contenders.find((x) => x.identity === identity)?.name;

/** Text as DPNS compares names (MG-07 D15): lower case, `o` → `0`, `i` and `l` → `1`, so a
 * search for "Alice" finds "a11ce". */
export const normalizeLabel = (text: string) => text.toLowerCase().replace(/o/g, "0").replace(/[il]/g, "1");

export function sameChoice(a: NameChoice | null | undefined, b: NameChoice | null | undefined): boolean {
  if (!a || !b) return false;
  if (a.kind === "towardsIdentity") return b.kind === "towardsIdentity" && a.identity === b.identity;
  return a.kind === b.kind;
}

/** "Alice (8ZtW…R97P)", "8ZtW…R97P", "Abstain", "Lock". */
export function choiceText(c: NameChoice, contest?: Pick<ContestedName, "contenders">): string {
  switch (c.kind) {
    case "towardsIdentity":
      return contenderText(c.identity, nameIn(c.identity, contest));
    case "lock":
      return t(`${P}.lock`);
    case "abstain":
      return t(`${P}.abstain`);
  }
}

/** The vote button's label: "Vote for Alice (8ZtW…R97P)", "Vote Lock", "Vote Abstain". */
export function voteButtonText(c: NameChoice | null, contest?: Pick<ContestedName, "contenders">): string {
  if (c === null) return t(`${P}.vote`);
  if (c.kind === "towardsIdentity") return t(`${P}.voteFor`, { choice: choiceText(c, contest) });
  return c.kind === "lock" ? t(`${P}.voteLock`) : t(`${P}.voteAbstain`);
}

/** A choice as a stable key, for radio values and React keys. */
export const choiceKey = (c: NameChoice) => (c.kind === "towardsIdentity" ? `id:${c.identity}` : c.kind);

/** The voter's standing in `label`'s contest; no state is no choice and no vote seen. */
export function voterState(v: NameVoter, label: string): NameVoterState {
  return (
    v.current.find((s) => s.label === label) ?? {
      label,
      choice: null,
      votesUsed: 0,
    }
  );
}

/** "3 votes left", "One vote left", "No votes left" (Android's wording; a lower bound, MG-07 D6). */
export function votesLeftText(used: number, allowed: number): string {
  const left = Math.max(0, allowed - used);
  return left === 0 ? t(`${P}.noVotesLeft`) : t(`${P}.votesLeft`, { count: left });
}

// ---------------------------------------------------------------- times

/** Voting on `c` is over by its end time, before Platform resolves it (MG-07 D8). */
export const isClosed = (c: Pick<ContestedName, "endsAt">, now: number) => now >= c.endsAt;

export const isOwnActiveContest = (c: ContestedName, now: number): boolean => c.own && !isClosed(c, now);

/** "Ends Jan 15 at 9:00 AM · in about 101 days", or "Voting ended Jan 15 at 9:00 AM". */
export function endsText(c: Pick<ContestedName, "endsAt">, now: number, locale?: string): string {
  if (isClosed(c, now)) return t(`${P}.endedAt`, { when: dateTime(c.endsAt, locale) });
  return t(`${P}.ends`, { when: dateTime(c.endsAt, locale), span: spanLong(c.endsAt - now) });
}

// ---------------------------------------------------------------- the tally

export interface TallyRowView {
  key: string;
  title: string;
  /** The full identity id for a contender (title and copy), else `null`. */
  identity: string | null;
  /** A contender shown by its name, not by its id alone (F1). */
  named: boolean;
  votes: number;
  /** Percent of every vote cast on the name. */
  pct: number;
  /** Takes the name if voting ended now (a contender over Lock, or Lock over every contender). */
  leading: boolean;
}

export interface TallyView {
  rows: TallyRowView[];
  total: number;
  /** "8ZtW…R97P leads with 9 of 14 votes", "Lock leads: …", "Tied: …", "No votes yet". */
  lead: string;
}

const votesOf = (n: number | null) => n ?? 0;
/** "1 vote", "14 votes". */
export const votesText = (n: number) => t(`${P}.votes`, { count: n });

/** The contenders by votes (Platform's order among equals), then Lock and Abstain, with who would
 * get the name if voting ended now: the contender with the most votes unless Lock has more
 * (Drive's `check_for_ended_vote_polls`); a tie among the top contenders names no one (D6). */
export function tallyView(c: ContestedName): TallyView {
  const contenders = c.contenders.map((x, i) => ({ ...x, i, v: votesOf(x.votes) })).sort((a, b) => b.v - a.v || a.i - b.i);
  const lock = votesOf(c.lockVotes);
  const abstain = votesOf(c.abstainVotes);
  const total = contenders.reduce((s, x) => s + x.v, 0) + lock + abstain;
  const pct = (n: number) => (total > 0 ? (n / total) * 100 : 0);
  const top = contenders[0]?.v ?? 0;
  const topCount = contenders.filter((x) => x.v === top).length;
  const tied = topCount > 1;
  const lockLeads = lock > top;
  const rows: TallyRowView[] = [
    ...contenders.map((x, i) => ({
      key: `id:${x.identity}`,
      title: contenderText(x.identity, x.name),
      identity: x.identity,
      named: !!x.name,
      votes: x.v,
      pct: pct(x.v),
      leading: i === 0 && top > 0 && !tied && !lockLeads,
    })),
    {
      key: "lock",
      title: t(`${P}.lock`),
      identity: null,
      named: false,
      votes: lock,
      pct: pct(lock),
      leading: lockLeads,
    },
    {
      key: "abstain",
      title: t(`${P}.abstain`),
      identity: null,
      named: false,
      votes: abstain,
      pct: pct(abstain),
      leading: false,
    },
  ];
  let lead: string;
  if (total === 0) lead = t(`${P}.lead.none`);
  else if (lockLeads) lead = t(`${P}.lead.lock`, { lock, count: total });
  else if (top === 0) lead = t(`${P}.lead.noContender`);
  else if (tied) lead = t(`${P}.lead.tied`, { count: top, contenders: topCount });
  else lead = t(`${P}.lead.leads`, { name: contenderText(contenders[0].identity, contenders[0].name), votes: top, count: total });
  return { rows, total, lead };
}

// ---------------------------------------------------------------- the list

export interface ContestRowView {
  label: string;
  sub: string;
  /** "You: Lock", "You: 1 of 3 voted", "You: no vote yet"; `null` without voters. */
  you: string | null;
  total: number;
  chip: ChipView | null;
}

/** "You: …" over the wallet's voters on `contest` (MG-09's "You:" line, D8). */
export function youText(voters: NameVoter[], contest: NameContest): string | null {
  if (voters.length === 0) return null;
  const choices = voters.map((v) => voterState(v, contest.label).choice);
  const voted = choices.filter((c) => c !== null);
  if (voted.length === 0) return t(`${P}.you.none`);
  if (voted.length === voters.length && voted.every((c) => sameChoice(c, voted[0]))) return t(`${P}.you.choice`, { choice: choiceText(voted[0], contest) });
  return t(`${P}.you.some`, { voted: voted.length, count: voters.length });
}

/** "Voting ended", or "Ends in …" within a day of the end; `null` otherwise (D7). */
export function contestChip(c: ContestedName, now: number, locale?: string): ChipView | null {
  if (isClosed(c, now)) return { text: t(`${P}.chip.ended`), tone: "neutral", tooltip: t(`${P}.chip.endedTooltip`) };
  const left = c.endsAt - now;
  if (left >= ENDING_SOON_SECS) return null;
  return { text: t(`${P}.chip.endsIn`, { span: spanLong(left) }), tone: "warning", tooltip: endsText(c, now, locale) };
}

export function contestRow(c: ContestedName, voters: NameVoter[], now: number, locale?: string): ContestRowView {
  const closed = isClosed(c, now);
  const key = `${P}.row.${closed ? "ended" : "ends"}${c.contendersComplete ? "" : "More"}` as const;
  return {
    label: c.label,
    sub: t(key, { count: c.contenders.length, date: shortDate(c.endsAt, locale) }),
    you: youText(voters, c),
    total: tallyView(c).total,
    chip: contestChip(c, now, locale),
  };
}

export type NameSort = "ending" | "name" | "votes" | "lock";
export const NAME_SORTS: { id: NameSort; readonly title: string }[] = (["ending", "name", "votes", "lock"] as const).map((id) => ({
  id,
  get title() {
    return t(`${P}.sort.${id}`);
  },
}));

/** iOS's sorts over the search (D4); "ending" keeps the engine's order, soonest first. */
export function listContests(contests: ContestedName[], search: string, sort: NameSort, now: number): ContestedName[] {
  const q = normalizeLabel(search.trim());
  const out = contests.filter((c) => q === "" || c.label.includes(q));
  if (sort === "name") out.sort((a, b) => a.label.localeCompare(b.label));
  else if (sort === "votes") out.sort((a, b) => tallyView(b).total - tallyView(a).total);
  else if (sort === "lock") out.sort((a, b) => votesOf(b.lockVotes) - votesOf(a.lockVotes));
  const own: ContestedName[] = [];
  const rest: ContestedName[] = [];
  for (const contest of out) (isOwnActiveContest(contest, now) ? own : rest).push(contest);
  return own.concat(rest);
}

/** Why Vote is off on a contest's pane, or `null` (D10). */
export function voteBlocked(c: ContestedName, names: ContestedNames, watchOnly: boolean, now: number): string | null {
  if (watchOnly) return NameText.voteWatchOnly;
  if (isClosed(c, now)) return NameText.ended;
  if (names.voters.length === 0) return NameText.noVoters;
  if (names.voters.every((v) => voterState(v, c.label).votesUsed >= names.votesAllowed)) return t(`${P}.allUsed`);
  return null;
}

/** The pane's "Your masternodes" line for one voter: its present choice and votes left. */
export function voterLine(v: NameVoter, contest: NameContest, allowed: number): string {
  const s = voterState(v, contest.label);
  const present = s.choice ? t(`${P}.votesChoice`, { choice: choiceText(s.choice, contest) }) : t(`${P}.noVoteYet`);
  return t(`${P}.voterLine`, { present, left: votesLeftText(s.votesUsed, allowed) });
}

// ---------------------------------------------------------------- the vote sheet

/** The choices a vote can name: the listed contenders, then Abstain and Lock (MG-00's
 * follow-up for MG-11), with each one's votes. */
export function choiceOptions(c: ContestedName): {
  choice: NameChoice;
  title: string;
  identity: string | null;
  named: boolean;
  votes: number;
}[] {
  return [
    ...c.contenders.map((x) => ({
      choice: { kind: "towardsIdentity", identity: x.identity } as NameChoice,
      title: contenderText(x.identity, x.name),
      identity: x.identity,
      named: !!x.name,
      votes: votesOf(x.votes),
    })),
    {
      choice: { kind: "abstain" },
      title: t(`${P}.abstain`),
      identity: null,
      named: false,
      votes: votesOf(c.abstainVotes),
    },
    {
      choice: { kind: "lock" },
      title: t(`${P}.lock`),
      identity: null,
      named: false,
      votes: votesOf(c.lockVotes),
    },
  ];
}

export interface NameVoterRowView {
  proTxHash: string;
  name: string;
  sub: string;
  weight: number;
  /** Its present choice, "None" without one. */
  present: string;
  /** Can take part with `choice` (the checkbox is on). */
  eligible: boolean;
  /** Votes it has left on this name. */
  votesLeft: number;
}

/** One row per voter (§4.25.4's rows, D11): the key's custody and votes left; a voter that used its
 * votes on this name is off (MG-07 D6: the engine refuses it at the limit), and so is one whose
 * present choice is `choice` already (Platform answers `already_voted`, MG-07 D18). A PoSe-banned
 * masternode still votes on names (Drive looks it up in its full list, D12). A voter the wallet has a
 * label for is named by it (MG-09 F3). */
export function nameVoterRows(voters: NameVoter[], contest: NameContest, choice: NameChoice | null, allowed: number, names?: NodeNames): NameVoterRowView[] {
  return voters.map((v) => {
    const s = voterState(v, contest.label);
    const custody = v.key.kind === "keychain" ? t(`${P}.sheet.keychain`, { index: v.key.index + 1 }) : t(`${P}.sheet.added`);
    const votesLeft = Math.max(0, allowed - s.votesUsed);
    const base = { proTxHash: v.proTxHash, name: nodeName(v, names), weight: v.weight, present: s.choice ? choiceText(s.choice, contest) : t(`${P}.sheet.none`), votesLeft };
    if (s.votesUsed >= allowed) return { ...base, sub: t(`${P}.sheet.usedAll`, { count: allowed }), eligible: false };
    if (sameChoice(s.choice, choice)) return { ...base, sub: t(`${P}.sheet.already`, { choice: choiceText(s.choice!, contest) }), eligible: false };
    return { ...base, sub: t(v.valid ? `${P}.sheet.eligible` : `${P}.sheet.eligibleBanned`, { custody, left: votesLeftText(s.votesUsed, allowed) }), eligible: true };
  });
}

/** The voters checked when the sheet opens (MG-00 D12): every one with a vote left. */
export const defaultVoters = (voters: NameVoter[], label: string, allowed: number) =>
  new Set(voters.filter((v) => voterState(v, label).votesUsed < allowed).map((v) => v.proTxHash));

/** The rows a vote goes out for: checked and eligible with the choice, so a voter at its limit or
 * on the choice already is never sent (D11). */
export const pickedVoters = (rows: NameVoterRowView[], checked: ReadonlySet<string>) => rows.filter((r) => r.eligible && checked.has(r.proTxHash));

/** The notice for checked voters casting their last vote (Android's "One vote left", D11). */
export function lastVoteNotice(picked: NameVoterRowView[]): string | null {
  const last = picked.filter((r) => r.votesLeft === 1);
  if (last.length === 0) return null;
  const names = last.map((r) => r.name).reduce((first, next) => t(`${P}.sheet.and`, { first, next }));
  return t(last.length === 1 ? `${P}.sheet.lastOne` : `${P}.sheet.lastMany`, { names });
}

// ---------------------------------------------------------------- the result

/** A sentence per reason dw-app passes (`NameRejectionName`, MG-11 D15): `unconfirmed` has its own. */
const REJECTED = messageTable(`${P}.result.rejected`, [
  "unknown_masternode",
  "no_voting_key",
  "cancelled",
  "signer_unavailable",
  "in_flight",
  "vote_limit",
  "platform_unavailable",
  "already_voted",
  "contest_closed",
  "contest_not_found",
  "masternode_not_found",
  "wrong_voting_key",
  "choice_not_allowed",
  "no_voter_identity",
  "voting_key_disabled",
  "refused",
  // A reason this build doesn't know (dw-app logs it): not counted, and not passed off as Platform's
  // `refused` (MG-11 review n4).
  "unknown",
] as const satisfies readonly Exclude<NameRejectionName, "unconfirmed">[]);

/** A `NameVoteCast.rejected` reason as a sentence (MG-09 D13's rule): never the raw value. */
export const nameRejectedText = (reason: Exclude<NameRejectionName, "unconfirmed">) => (Object.hasOwn(REJECTED, reason) ? REJECTED[reason] : REJECTED.unknown);

/** The sentence for an outcome Platform didn't answer (MG-07 D7): never a success claim. */
export const unconfirmedText = () => t(`${P}.result.unconfirmed`);

export type CastState = "counted" | "unconfirmed" | "refused";

export interface NameResultView {
  title: string;
  line: string;
  rows: {
    proTxHash: string;
    name: string;
    sub: string;
    state: CastState;
    chip: ChipView;
  }[];
  /** Some vote is unconfirmed: offer Check again (D14). */
  unconfirmed: boolean;
}

const TONE: Record<CastState, ChipView["tone"]> = { counted: "success", unconfirmed: "neutral", refused: "neutral" };
const chip = (state: CastState): ChipView => ({ text: t(`${P}.result.chip.${state}`), tone: TONE[state], tooltip: "" });

/** §4.25.4's result for a name vote: "2 of 3 votes counted", the choice with its weight, and one
 * line per masternode. An `unconfirmed` vote is never counted (MG-07 D7) until a re-read
 * (`reread`, the contests read after the vote) shows its masternode's present choice is the one
 * cast (D14). */
export function nameVoteResult(casts: NameVoteCast[], voters: NameVoter[], contest: NameContest, choice: NameChoice, reread?: ContestedNames, names?: NodeNames): NameResultView {
  const { label } = contest;
  const byHash = new Map(voters.map((v) => [v.proTxHash, v]));
  const now = new Map((reread?.voters ?? []).map((v) => [v.proTxHash, voterState(v, label).choice]));
  const weightOf = (h: string) => byHash.get(h)?.weight ?? 1;
  const rows = casts.map((c) => {
    const v = byHash.get(c.proTxHash);
    const name = v ? nodeName(v, names) : t(`${P}.result.masternode`, { hash: shortHash(c.proTxHash) });
    const weight = weightOf(c.proTxHash);
    let state: CastState, sub: string;
    if (c.rejected === null) [state, sub] = ["counted", t(`${P}.result.counted`, { weight })];
    else if (c.rejected !== "unconfirmed") [state, sub] = ["refused", nameRejectedText(c.rejected)];
    else if (sameChoice(now.get(c.proTxHash), choice)) [state, sub] = ["counted", t(`${P}.result.nowShows`, { weight })];
    else [state, sub] = ["unconfirmed", unconfirmedText()];
    return { proTxHash: c.proTxHash, name, sub, state, chip: chip(state) };
  });
  const counted = rows.filter((r) => r.state === "counted");
  const pending = rows.filter((r) => r.state === "unconfirmed").length;
  const total = casts.reduce((s, c) => s + weightOf(c.proTxHash), 0);
  const weight = counted.reduce((s, r) => s + weightOf(r.proTxHash), 0);
  return {
    title: t(pending > 0 ? `${P}.result.titlePending` : `${P}.result.title`, { counted: counted.length, count: casts.length, pending }),
    line:
      choice.kind === "towardsIdentity"
        ? t(`${P}.result.lineFor`, { identity: contenderText(choice.identity, nameIn(choice.identity, contest)), label, weight, total })
        : t(`${P}.result.line`, { choice: choiceText(choice, contest), label, weight, total }),
    rows,
    unconfirmed: pending > 0,
  };
}

// ---------------------------------------------------------------- errors

/** A contested-name call's error as a sentence (D13); `null` when it is not a governance code, so
 * the caller can try the send and Platform sentences (`screens/pay/model.ts` `failureText`). */
export function nameErrorText(e: unknown): string | null {
  if (!isAppError(e)) return null;
  const label = e.data?.kind === "contestLabel" ? e.data.label : null;
  switch (e.code) {
    case "governance.contest_not_found":
      return label ? t(`${P}.errors.contestNotFoundLabel`, { label }) : t(`${P}.errors.contestNotFound`);
    case "governance.contest_closed":
      return label ? t(`${P}.errors.contestClosedLabel`, { label }) : t(`${P}.errors.contestClosed`);
    case "governance.not_a_contender":
      return e.data?.kind === "contender" ? t(`${P}.errors.notAContenderIdentity`, { identity: shortIdentity(e.data.identity) }) : t(`${P}.errors.notAContender`);
    case "platform.unavailable":
    case "platform.timeout":
      return t(`${P}.errors.platformUnavailable`);
    default:
      return governanceErrorText(e);
  }
}
