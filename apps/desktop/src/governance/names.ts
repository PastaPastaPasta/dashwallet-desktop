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
import { governanceErrorText, nodeName, shortDate, shortHash, spanLong, type ChipView } from "./model";

/** A contest ending within this reads "Ends in …" on its row (D7). */
const ENDING_SOON_SECS = 86_400;

export const NameText = {
  tab: "Username voting",
  intro:
    "When more than one identity asks for the same username, masternodes vote on who gets it. When voting ends, the contender with the most votes gets the name, unless Lock has more votes: then nobody does.",
  search: "Search usernames",
  browsingOnly: "Browsing only.",
  browsingOnlyText: "This wallet holds no masternode voting keys, so it can read these votes but not cast them.",
  watchOnly: "Watch-only wallet.",
  watchOnlyText: "It shows the votes on usernames. Voting needs the wallet that holds the voting keys.",
  voteWatchOnly: "Voting needs the wallet that holds the voting keys",
  noVoters: "No masternode in this wallet can vote on usernames",
  empty: "No usernames are being voted on",
  emptyHelp: "A vote starts when a second identity asks for a name someone has asked for.",
  loading: "Reading the votes on usernames from Platform…",
  failed: "Couldn't read the votes on usernames",
  /** A later read failed: the tab keeps the last one it had (D25). */
  stale: "These votes may be out of date.",
  ended: "Voting on this name has ended",
  tryAgain: "Try again",
  limitNote: (allowed: number) =>
    `Each masternode has ${allowed} votes per username: its first vote and ${allowed - 1} changes. This wallet counts the votes it sent, so votes sent from another wallet aren't included.`,
  privacy: "Voting with several masternodes together shows that one person controls them. Choose fewer to keep them unlinked.",
  partial: "Platform lists the first 100 contenders of this name. Only those can get votes from this wallet.",
  normalized: "Usernames are shown as Platform compares them: o reads as 0, and i and l as 1.",
  counts: "The tally updates as Platform takes the votes.",
} as const;

// ---------------------------------------------------------------- names and choices

/** "8ZtW…R97P": an identity id (base58) the facade carries without a name (D5). */
export const shortIdentity = (id: string) => (id.length > 12 ? `${id.slice(0, 4)}…${id.slice(-4)}` : id);

/** Text as DPNS compares names (MG-07 D15): lower case, `o` → `0`, `i` and `l` → `1`, so a
 * search for "Alice" finds "a11ce". */
export const normalizeLabel = (text: string) => text.toLowerCase().replace(/o/g, "0").replace(/[il]/g, "1");

export function sameChoice(a: NameChoice | null | undefined, b: NameChoice | null | undefined): boolean {
  if (!a || !b) return false;
  if (a.kind === "towardsIdentity") return b.kind === "towardsIdentity" && a.identity === b.identity;
  return a.kind === b.kind;
}

/** "8ZtW…R97P", "Abstain", "Lock". */
export function choiceText(c: NameChoice): string {
  switch (c.kind) {
    case "towardsIdentity":
      return shortIdentity(c.identity);
    case "lock":
      return "Lock";
    case "abstain":
      return "Abstain";
  }
}

/** The vote button's label: "Vote for 8ZtW…R97P", "Vote Lock", "Vote Abstain". */
export function voteButtonText(c: NameChoice | null): string {
  if (c === null) return "Vote";
  return c.kind === "towardsIdentity" ? `Vote for ${choiceText(c)}` : `Vote ${choiceText(c)}`;
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
  return left === 0 ? "No votes left" : left === 1 ? "One vote left" : `${left} votes left`;
}

// ---------------------------------------------------------------- times

/** Voting on `c` is over by its end time, before Platform resolves it (MG-07 D8). */
export const isClosed = (c: Pick<ContestedName, "endsAt">, now: number) => now >= c.endsAt;

function dateTime(secs: number, locale?: string): string {
  const t = new Date(secs * 1000).toLocaleTimeString(locale, {
    hour: "numeric",
    minute: "2-digit",
  });
  return `${shortDate(secs, locale)} at ${t}`;
}

/** "Ends Jan 15 at 9:00 AM · in about 101 days", or "Voting ended Jan 15 at 9:00 AM". */
export function endsText(c: Pick<ContestedName, "endsAt">, now: number, locale?: string): string {
  if (isClosed(c, now)) return `Voting ended ${dateTime(c.endsAt, locale)}`;
  return `Ends ${dateTime(c.endsAt, locale)} · in about ${spanLong(c.endsAt - now)}`;
}

// ---------------------------------------------------------------- the tally

export interface TallyRowView {
  key: string;
  title: string;
  /** The full identity id for a contender (title and copy), else `null`. */
  identity: string | null;
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
const plural = (n: number, one: string, many = `${one}s`) => `${n.toLocaleString("en-US")} ${n === 1 ? one : many}`;

/** "1 vote", "14 votes". */
export const votesText = (n: number) => plural(n, "vote");

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
      title: shortIdentity(x.identity),
      identity: x.identity,
      votes: x.v,
      pct: pct(x.v),
      leading: i === 0 && top > 0 && !tied && !lockLeads,
    })),
    {
      key: "lock",
      title: "Lock",
      identity: null,
      votes: lock,
      pct: pct(lock),
      leading: lockLeads,
    },
    {
      key: "abstain",
      title: "Abstain",
      identity: null,
      votes: abstain,
      pct: pct(abstain),
      leading: false,
    },
  ];
  let lead: string;
  if (total === 0) lead = "No votes yet";
  else if (lockLeads) lead = `Lock leads with ${lock.toLocaleString("en-US")} of ${votesText(total)}: if voting ended now, nobody would get the name`;
  else if (top === 0) lead = "No contender has a vote yet";
  else if (tied) lead = `Tied at ${votesText(top)} between ${topCount} contenders`;
  else lead = `${shortIdentity(contenders[0].identity)} leads with ${top.toLocaleString("en-US")} of ${votesText(total)}`;
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

/** "You: …" over the wallet's voters on `label` (MG-09's "You:" line, D8). */
export function youText(voters: NameVoter[], label: string): string | null {
  if (voters.length === 0) return null;
  const choices = voters.map((v) => voterState(v, label).choice);
  const voted = choices.filter((c) => c !== null);
  if (voted.length === 0) return "You: no vote yet";
  if (voted.length === voters.length && voted.every((c) => sameChoice(c, voted[0]))) return `You: ${choiceText(voted[0])}`;
  return `You: ${voted.length} of ${voters.length} voted`;
}

/** "Voting ended", or "Ends in …" within a day of the end; `null` otherwise (D7). */
export function contestChip(c: ContestedName, now: number, locale?: string): ChipView | null {
  if (isClosed(c, now)) return { text: "Voting ended", tone: "neutral", tooltip: "Platform names the winner in its next block." };
  const left = c.endsAt - now;
  if (left >= ENDING_SOON_SECS) return null;
  return { text: `Ends in ${spanLong(left)}`, tone: "warning", tooltip: endsText(c, now, locale) };
}

export function contestRow(c: ContestedName, voters: NameVoter[], now: number, locale?: string): ContestRowView {
  const closed = isClosed(c, now);
  const count = c.contendersComplete ? plural(c.contenders.length, "contender") : `${c.contenders.length}+ contenders`;
  return {
    label: c.label,
    sub: `${count} · ${closed ? "ended" : "ends"} ${shortDate(c.endsAt, locale)}`,
    you: youText(voters, c.label),
    total: tallyView(c).total,
    chip: contestChip(c, now, locale),
  };
}

export type NameSort = "ending" | "name" | "votes" | "lock";
export const NAME_SORTS: { id: NameSort; title: string }[] = [
  { id: "ending", title: "Ending soonest" },
  { id: "name", title: "Name" },
  { id: "votes", title: "Most votes" },
  { id: "lock", title: "Most lock votes" },
];

/** iOS's sorts over the search (D4); "ending" keeps the engine's order, soonest first. */
export function listContests(contests: ContestedName[], search: string, sort: NameSort): ContestedName[] {
  const q = normalizeLabel(search.trim());
  const out = contests.filter((c) => q === "" || c.label.includes(q));
  if (sort === "name") out.sort((a, b) => a.label.localeCompare(b.label));
  else if (sort === "votes") out.sort((a, b) => tallyView(b).total - tallyView(a).total);
  else if (sort === "lock") out.sort((a, b) => votesOf(b.lockVotes) - votesOf(a.lockVotes));
  return out;
}

/** Why Vote is off on a contest's pane, or `null` (D10). */
export function voteBlocked(c: ContestedName, names: ContestedNames, watchOnly: boolean, now: number): string | null {
  if (watchOnly) return NameText.voteWatchOnly;
  if (isClosed(c, now)) return NameText.ended;
  if (names.voters.length === 0) return NameText.noVoters;
  if (names.voters.every((v) => voterState(v, c.label).votesUsed >= names.votesAllowed)) return "Your masternodes have used all their votes on this name";
  return null;
}

/** The pane's "Your masternodes" line for one voter: its present choice and votes left. */
export function voterLine(v: NameVoter, label: string, allowed: number): string {
  const s = voterState(v, label);
  const present = s.choice ? `Votes ${choiceText(s.choice)}` : "No vote yet";
  return `${present} · ${votesLeftText(s.votesUsed, allowed)}`;
}

// ---------------------------------------------------------------- the vote sheet

/** The choices a vote can name: the listed contenders, then Abstain and Lock (MG-00's
 * follow-up for MG-11), with each one's votes. */
export function choiceOptions(c: ContestedName): {
  choice: NameChoice;
  title: string;
  identity: string | null;
  votes: number;
}[] {
  return [
    ...c.contenders.map((x) => ({
      choice: { kind: "towardsIdentity", identity: x.identity } as NameChoice,
      title: shortIdentity(x.identity),
      identity: x.identity,
      votes: votesOf(x.votes),
    })),
    {
      choice: { kind: "abstain" },
      title: "Abstain",
      identity: null,
      votes: votesOf(c.abstainVotes),
    },
    {
      choice: { kind: "lock" },
      title: "Lock",
      identity: null,
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
 * masternode still votes on names (Drive looks it up in its full list, D12). */
export function nameVoterRows(voters: NameVoter[], label: string, choice: NameChoice | null, allowed: number): NameVoterRowView[] {
  return voters.map((v) => {
    const s = voterState(v, label);
    const custody = v.key.kind === "keychain" ? `key in this wallet · voting key ${v.key.index + 1}` : "key you added";
    const banned = v.valid ? "" : " · PoSe banned, still votes on names";
    const votesLeft = Math.max(0, allowed - s.votesUsed);
    const base = { proTxHash: v.proTxHash, name: nodeName(v), weight: v.weight, present: s.choice ? choiceText(s.choice) : "None", votesLeft };
    if (s.votesUsed >= allowed) return { ...base, sub: `No votes left: it used its ${allowed} votes on this name`, eligible: false };
    if (sameChoice(s.choice, choice)) return { ...base, sub: `Votes ${choiceText(s.choice!)} already`, eligible: false };
    return { ...base, sub: `${custody}${banned} · ${votesLeftText(s.votesUsed, allowed)}`, eligible: true };
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
  const names = last.map((r) => r.name).join(" and ");
  return last.length === 1
    ? `This is the last vote ${names} has on this name: after it, its choice can't change.`
    : `This is the last vote ${names} have on this name: after it, their choices can't change.`;
}

// ---------------------------------------------------------------- the result

/** A sentence per reason dw-app passes (`NameRejectionName`, MG-11 D15): `unconfirmed` has its own. */
const REJECTED: Record<Exclude<NameRejectionName, "unconfirmed">, string> = {
  unknown_masternode: "This masternode isn't in the current masternode list.",
  no_voting_key: "Its voting key is not in this wallet.",
  cancelled: "Not sent: the wallet locked, or the send ran out of time, before it left.",
  signer_unavailable: "The wallet couldn't read its voting key.",
  in_flight: "The same vote is already being sent.",
  vote_limit: "It has used all its votes on this name, so its choice can't change.",
  platform_unavailable: "Not sent: Platform couldn't be reached.",
  already_voted: "Platform already has this choice from it, so nothing changed.",
  contest_closed: "Voting on this name has ended.",
  contest_not_found: "Platform has no vote on this name.",
  masternode_not_found: "Platform's masternode list doesn't have it yet.",
  wrong_voting_key: "Its voting key changed, so the key in this wallet no longer matches.",
  choice_not_allowed: "Platform doesn't allow this choice on this name.",
  no_voter_identity: "Platform has no voting identity for its key yet.",
  voting_key_disabled: "Platform disabled this voting key: the masternode's voting key was changed.",
  refused: "Platform refused the vote.",
  // A reason this build doesn't know (dw-app logs it): not counted, and not passed off as Platform's
  // `refused` (MG-11 review n4).
  unknown: "Not counted, for a reason this version of the wallet doesn't know.",
};

/** A `NameVoteCast.rejected` reason as a sentence (MG-09 D13's rule): never the raw value. */
export const nameRejectedText = (reason: Exclude<NameRejectionName, "unconfirmed">) => REJECTED[reason];

/** The sentence for an outcome Platform didn't answer (MG-07 D7): never a success claim. */
export const UNCONFIRMED_TEXT = "Sent, but Platform didn't confirm it. It may still count: check again in a minute.";

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

const CHIPS: Record<CastState, ChipView> = {
  counted: { text: "Counted", tone: "success", tooltip: "" },
  unconfirmed: { text: "Unconfirmed", tone: "neutral", tooltip: "" },
  refused: { text: "Not counted", tone: "neutral", tooltip: "" },
};

/** §4.25.4's result for a name vote: "2 of 3 votes counted", the choice with its weight, and one
 * line per masternode. An `unconfirmed` vote is never counted (MG-07 D7) until a re-read
 * (`reread`, the contests read after the vote) shows its masternode's present choice is the one
 * cast (D14). */
export function nameVoteResult(casts: NameVoteCast[], voters: NameVoter[], label: string, choice: NameChoice, reread?: ContestedNames): NameResultView {
  const byHash = new Map(voters.map((v) => [v.proTxHash, v]));
  const now = new Map((reread?.voters ?? []).map((v) => [v.proTxHash, voterState(v, label).choice]));
  const weightOf = (h: string) => byHash.get(h)?.weight ?? 1;
  const rows = casts.map((c) => {
    const v = byHash.get(c.proTxHash);
    const name = v ? nodeName(v) : `Masternode ${shortHash(c.proTxHash)}`;
    const weight = `weight ${weightOf(c.proTxHash)}`;
    let state: CastState, sub: string;
    if (c.rejected === null) [state, sub] = ["counted", `${weight} · Platform counted it`];
    else if (c.rejected !== "unconfirmed") [state, sub] = ["refused", nameRejectedText(c.rejected)];
    else if (sameChoice(now.get(c.proTxHash), choice)) [state, sub] = ["counted", `${weight} · Platform now shows this choice`];
    else [state, sub] = ["unconfirmed", UNCONFIRMED_TEXT];
    return { proTxHash: c.proTxHash, name, sub, state, chip: CHIPS[state] };
  });
  const counted = rows.filter((r) => r.state === "counted");
  const pending = rows.filter((r) => r.state === "unconfirmed").length;
  const total = casts.reduce((s, c) => s + weightOf(c.proTxHash), 0);
  const weight = counted.reduce((s, r) => s + weightOf(r.proTxHash), 0);
  return {
    title: `${counted.length} of ${votesText(casts.length)} counted${pending > 0 ? ` · ${pending} not confirmed yet` : ""}`,
    line: `${choice.kind === "towardsIdentity" ? `For ${shortIdentity(choice.identity)}` : choiceText(choice)} on ${label} · weight ${weight} of ${total}`,
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
      return label ? `Platform has no vote on “${label}”.` : "Platform has no vote on this name.";
    case "governance.contest_closed":
      return label ? `Voting on “${label}” has ended.` : "Voting on this name has ended.";
    case "governance.not_a_contender":
      return e.data?.kind === "contender" ? `${shortIdentity(e.data.identity)} isn't a contender for this name.` : "That identity isn't a contender for this name.";
    case "platform.unavailable":
    case "platform.timeout":
      return "Couldn't reach Dash Platform. Try again in a moment.";
    default:
      return governanceErrorText(e);
  }
}
