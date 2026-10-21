// Username voting's view model (names.ts, MG-11) on MG-07's fixture: the tally and who leads by
// Drive's rule, the list's rows, search and sorts, the voters a vote may go out for, and the result
// with every `rejected` reason, an `unconfirmed` vote never counted until a re-read shows it.

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { AppError } from "../api/generated/AppError";
import type { ContestedName } from "../api/generated/ContestedName";
import type { ContestedNames } from "../api/generated/ContestedNames";
import type { NameRejectionName } from "../api/generated/NameRejectionName";
import type { NameVoteCast } from "../api/generated/NameVoteCast";
import { FIXTURE_NAMES, ID_1, ID_2, VOTER_A, VOTER_B, VOTER_C } from "../fixture/names";
import { FIXTURE_NOW, secs } from "../fixture/wallet";
import {
  choiceOptions,
  contestRow,
  defaultVoters,
  endsText,
  isClosed,
  lastVoteNotice,
  listContests,
  nameErrorText,
  nameRejectedText,
  nameVoteResult,
  nameVoterRows,
  normalizeLabel,
  pickedVoters,
  sameChoice,
  shortIdentity,
  tallyView,
  UNCONFIRMED_TEXT,
  voteBlocked,
  voteButtonText,
  voterLine,
  votesLeftText,
  youText,
} from "./names";

const NOW = secs(FIXTURE_NOW);
const VECTORS = JSON.parse(readFileSync(new URL("../fixture/vectors/governance.json", import.meta.url), "utf8"));
const names = (): ContestedNames => structuredClone(FIXTURE_NAMES);
const a11ce = (n = names()) => n.contests[0];
const towards = (identity: string) => ({ kind: "towardsIdentity", identity }) as const;
const LOCK = { kind: "lock" } as const;
const ABSTAIN = { kind: "abstain" } as const;
const cast = (proTxHash: string, rejected: NameRejectionName | null): NameVoteCast => ({ proTxHash, transition: rejected === "vote_limit" ? null : "5e".repeat(32), rejected });

describe("names and choices", () => {
  it("compares names as DPNS does", () => {
    expect(normalizeLabel("Alice")).toBe("a11ce");
    expect(normalizeLabel("DASH-Pay")).toBe("dash-pay");
    expect(listContests(names().contests, "ALICE", "ending").map((c) => c.label)).toEqual(["a11ce"]);
    expect(listContests(names().contests, "  ", "ending")).toHaveLength(2);
    expect(listContests(names().contests, "bob", "ending")).toEqual([]);
  });

  it("names a choice and its button, and tells choices apart", () => {
    expect(shortIdentity(ID_1)).toBe("8ZtW…R97P");
    expect(voteButtonText(null)).toBe("Vote");
    expect(voteButtonText(towards(ID_1))).toBe("Vote for 8ZtW…R97P");
    expect(voteButtonText(LOCK)).toBe("Vote Lock");
    expect(sameChoice(towards(ID_1), towards(ID_1))).toBe(true);
    expect(sameChoice(towards(ID_1), towards(ID_2))).toBe(false);
    expect(sameChoice(LOCK, LOCK)).toBe(true);
    expect(sameChoice(null, LOCK)).toBe(false);
    // The listed contenders, then Abstain and Lock, each with its votes.
    expect(choiceOptions(a11ce()).map((o) => [o.title, o.votes])).toEqual([
      ["8ZtW…R97P", 9],
      ["4EfA…iSeF", 4],
      ["Abstain", 0],
      ["Lock", 1],
    ]);
  });

  it("words the votes left as Android does", () => {
    expect([0, 3, 4, 5, 7].map((used) => votesLeftText(used, 5))).toEqual(["5 votes left", "2 votes left", "One vote left", "No votes left", "No votes left"]);
  });
});

describe("the tally", () => {
  it("leads with the contender with most votes", () => {
    const t = tallyView(a11ce());
    expect(t.total).toBe(14);
    expect(t.rows.map((r) => [r.key, r.votes, r.leading])).toEqual([
      [`id:${ID_1}`, 9, true],
      [`id:${ID_2}`, 4, false],
      ["lock", 1, false],
      ["abstain", 0, false],
    ]);
    expect(t.rows[0].pct).toBeCloseTo((9 / 14) * 100);
    expect(t.lead).toBe("8ZtW…R97P leads with 9 of 14 votes");
  });

  it("lets Lock lead only with strictly more votes than every contender (Drive)", () => {
    const c = a11ce();
    c.lockVotes = 9;
    // A tie with the top contender: the contender still leads.
    expect(tallyView(c).rows.find((r) => r.leading)?.key).toBe(`id:${ID_1}`);
    c.lockVotes = 10;
    const t = tallyView(c);
    expect(t.rows.filter((r) => r.leading).map((r) => r.key)).toEqual(["lock"]);
    expect(t.lead).toBe("Lock leads with 10 of 23 votes: if voting ended now, nobody would get the name");
  });

  it("names no one in a tie or without votes, and reads a missing count as none", () => {
    const c = a11ce();
    c.contenders[1].votes = 9;
    expect(tallyView(c).rows.some((r) => r.leading)).toBe(false);
    expect(tallyView(c).lead).toBe("Tied at 9 votes between 2 contenders");
    const empty: ContestedName = { ...c, contenders: c.contenders.map((x) => ({ ...x, votes: null })), lockVotes: null, abstainVotes: null };
    expect(tallyView(empty)).toMatchObject({ total: 0, lead: "No votes yet" });
    expect(tallyView({ ...empty, abstainVotes: 2 }).lead).toBe("No contender has a vote yet");
  });
});

describe("the list", () => {
  it("shows each contest's contenders, end, your votes and an ending chip", () => {
    const n = names();
    const row = contestRow(a11ce(n), n.voters, NOW);
    expect(row).toMatchObject({ label: "a11ce", you: "You: 2 of 3 voted", total: 14, chip: null });
    expect(row.sub).toMatch(/^2 contenders · ends /);
    const soon = contestRow({ ...a11ce(n), endsAt: NOW + 3 * 3600 }, n.voters, NOW);
    expect(soon.chip).toMatchObject({ text: "Ends in 3 hours", tone: "warning" });
    const ended = contestRow({ ...a11ce(n), endsAt: NOW - 600 }, n.voters, NOW);
    expect(ended.chip).toMatchObject({ text: "Voting ended", tone: "neutral" });
    expect(ended.sub).toMatch(/· ended /);
    expect(contestRow({ ...a11ce(n), contendersComplete: false }, n.voters, NOW).sub).toMatch(/^2\+ contenders/);
    expect(contestRow(a11ce(n), [], NOW).you).toBeNull();
  });

  it("says You: in one choice, a share, or none", () => {
    const n = names();
    expect(youText(n.voters, "dash-pay")).toBe("You: 1 of 3 voted");
    expect(youText([n.voters[1]], "a11ce")).toBe("You: Lock");
    expect(youText([n.voters[2]], "a11ce")).toBe("You: no vote yet");
  });

  it("sorts as iOS does", () => {
    const n = names();
    const c = n.contests;
    const b0b = { ...c[1], label: "b0b", endsAt: c[1].endsAt + 1, lockVotes: 7 };
    const all = [...c, b0b];
    expect(listContests(all, "", "ending").map((x) => x.label)).toEqual(["a11ce", "dash-pay", "b0b"]);
    expect(listContests(all, "", "name").map((x) => x.label)).toEqual(["a11ce", "b0b", "dash-pay"]);
    expect(listContests(all, "", "votes").map((x) => x.label)).toEqual(["a11ce", "b0b", "dash-pay"]);
    expect(listContests(all, "", "lock").map((x) => x.label)).toEqual(["b0b", "a11ce", "dash-pay"]);
  });

  it("closes a contest at its end time and says when", () => {
    const c = a11ce();
    expect(isClosed(c, c.endsAt - 1)).toBe(false);
    expect(isClosed(c, c.endsAt)).toBe(true);
    expect(endsText(c, c.endsAt - 3 * 86_400)).toMatch(/^Ends .* at .* · in about 3 days$/);
    expect(endsText(c, c.endsAt)).toMatch(/^Voting ended .* at /);
  });

  it("blocks Vote watch-only, after the end, without voters or with every vote used", () => {
    const n = names();
    const c = a11ce(n);
    expect(voteBlocked(c, n, false, NOW)).toBeNull();
    expect(voteBlocked(c, n, true, NOW)).toBe("Voting needs the wallet that holds the voting keys");
    expect(voteBlocked(c, n, false, c.endsAt)).toBe("Voting on this name has ended");
    expect(voteBlocked(c, { ...n, voters: [] }, false, NOW)).toBe("No masternode in this wallet can vote on usernames");
    expect(voteBlocked(c, { ...n, voters: [n.voters[1]] }, false, NOW)).toBe("Your masternodes have used all their votes on this name");
    expect(voterLine(n.voters[1], "a11ce", 5)).toBe("Votes Lock · No votes left");
    expect(voterLine(n.voters[2], "a11ce", 5)).toBe("No vote yet · 5 votes left");
  });
});

describe("the vote sheet's voters", () => {
  it("offers every voter with a vote left, a banned one too, and says whose key it is", () => {
    const n = names();
    const rows = nameVoterRows(n.voters, "a11ce", null, n.votesAllowed);
    expect(rows.map((r) => [r.name, r.eligible, r.sub, r.present])).toEqual([
      ["Masternode a1a1a1a1", true, "key in this wallet · voting key 1 · 3 votes left", "8ZtW…R97P"],
      ["Evonode b2b2b2b2", false, "No votes left: it used its 5 votes on this name", "Lock"],
      ["Masternode c3c3c3c3", true, "key you added · PoSe banned, still votes on names · 5 votes left", "None"],
    ]);
    expect([...defaultVoters(n.voters, "a11ce", n.votesAllowed)]).toEqual([VOTER_A, VOTER_C]);
    expect([...defaultVoters(n.voters, "dash-pay", n.votesAllowed)]).toEqual([VOTER_A, VOTER_B, VOTER_C]);
  });

  // Mutant 1 (votes-used ignored at the limit) dies here: the evonode used its 5 votes on a11ce.
  it("never offers a voter at its vote limit, whatever the choice", () => {
    const n = names();
    for (const choice of [null, towards(ID_2), ABSTAIN, LOCK]) {
      const b = nameVoterRows(n.voters, "a11ce", choice, n.votesAllowed).find((r) => r.proTxHash === VOTER_B)!;
      expect(b.eligible, JSON.stringify(choice)).toBe(false);
    }
    // At 4 of 5 it may still vote, and the sheet says it is its last.
    n.voters[0].current[0].votesUsed = 4;
    const rows = nameVoterRows(n.voters, "a11ce", ABSTAIN, n.votesAllowed);
    expect(rows[0]).toMatchObject({ eligible: true, sub: "key in this wallet · voting key 1 · One vote left" });
    expect(lastVoteNotice(pickedVoters(rows, new Set([VOTER_A, VOTER_C])))).toBe("This is the last vote Masternode a1a1a1a1 has on this name: after it, its choice can't change.");
    expect(lastVoteNotice(pickedVoters(rows, new Set([VOTER_C])))).toBeNull();
  });

  it("turns off a voter whose present choice is the one picked (Platform's already_voted)", () => {
    const n = names();
    const rows = nameVoterRows(n.voters, "a11ce", towards(ID_1), n.votesAllowed);
    expect(rows[0]).toMatchObject({ eligible: false, sub: "Votes 8ZtW…R97P already" });
    expect(rows[2].eligible).toBe(true);
  });

  // Mutant 3 (an ineligible voter sent) dies here: a checked voter that can't vote is not picked.
  it("sends only checked voters that can vote with the choice", () => {
    const n = names();
    const all = new Set([VOTER_A, VOTER_B, VOTER_C]);
    expect(pickedVoters(nameVoterRows(n.voters, "a11ce", towards(ID_1), n.votesAllowed), all).map((r) => r.proTxHash)).toEqual([VOTER_C]);
    expect(pickedVoters(nameVoterRows(n.voters, "a11ce", ABSTAIN, n.votesAllowed), new Set([VOTER_B, VOTER_C])).map((r) => r.proTxHash)).toEqual([VOTER_C]);
    expect(pickedVoters(nameVoterRows(n.voters, "dash-pay", LOCK, n.votesAllowed), all).map((r) => r.proTxHash)).toEqual([VOTER_A, VOTER_B, VOTER_C]);
  });
});

describe("the result", () => {
  it("counts the counted votes with their weight", () => {
    const n = names();
    const view = nameVoteResult([cast(VOTER_A, null), cast(VOTER_C, null)], n.voters, "a11ce", towards(ID_2));
    expect(view.title).toBe("2 of 2 votes counted");
    expect(view.line).toBe("For 4EfA…iSeF on a11ce · weight 2 of 2");
    expect(view.rows.map((r) => [r.state, r.chip.text, r.chip.tone])).toEqual([
      ["counted", "Counted", "success"],
      ["counted", "Counted", "success"],
    ]);
    expect(view.unconfirmed).toBe(false);
  });

  // Mutant 2 (`unconfirmed` shown as success) dies here.
  it("never counts an unconfirmed vote until a re-read shows its choice", () => {
    const n = names();
    const casts = [cast(VOTER_A, "unconfirmed"), cast(VOTER_B, "vote_limit"), cast(VOTER_C, null)];
    const view = nameVoteResult(casts, n.voters, "a11ce", ABSTAIN);
    expect(view.title).toBe("1 of 3 votes counted · 1 not confirmed yet");
    expect(view.line).toBe("Abstain on a11ce · weight 1 of 6");
    expect(view.rows[0]).toMatchObject({ state: "unconfirmed", sub: UNCONFIRMED_TEXT, chip: { text: "Unconfirmed", tone: "neutral" } });
    expect(view.rows[1]).toMatchObject({ state: "refused", sub: "It has used all its votes on this name, so its choice can't change.", chip: { text: "Not counted" } });
    expect(view.unconfirmed).toBe(true);
    // A re-read that still shows the old choice keeps it unconfirmed.
    expect(nameVoteResult(casts, n.voters, "a11ce", ABSTAIN, names()).rows[0].state).toBe("unconfirmed");
    // One that shows the cast choice counts it.
    const after = names();
    after.voters[0].current[0].choice = ABSTAIN;
    const checked = nameVoteResult(casts, n.voters, "a11ce", ABSTAIN, after);
    expect(checked.rows[0]).toMatchObject({ state: "counted", sub: "weight 1 · Platform now shows this choice" });
    expect(checked.title).toBe("2 of 3 votes counted");
    expect(checked.unconfirmed).toBe(false);
  });

  it("has its own sentence for every rejected reason the engine sends, never the raw value", () => {
    // `REJECTED` is typed by dw-app's `NameRejectionName`, so tsc holds the map to the union; the
    // vectors (MG-07's fixture: every reason once) hold the union to the engine.
    const reasons = (VECTORS.results.governance_vote_name as NameVoteCast[])
      .map((c) => c.rejected)
      .filter((r): r is Exclude<NameRejectionName, "unconfirmed"> => r !== null && r !== "unconfirmed");
    expect(reasons).toHaveLength(16);
    for (const r of reasons) expect(nameRejectedText(r), r).not.toMatch(/_|^[a-z]+$/);
    expect(new Set(reasons.map(nameRejectedText)).size).toBe(reasons.length);
    // An unknown masternode in the result is named by its hash.
    const view = nameVoteResult([cast("d4".repeat(32), "unknown_masternode")], names().voters, "a11ce", LOCK);
    expect(view.rows[0]).toMatchObject({ name: "Masternode d4d4d4d4", state: "refused" });
  });
});

describe("errors", () => {
  it("names the contest or the contender", () => {
    const err = (code: AppError["code"], data: AppError["data"]): AppError => ({ code, message: "x", data });
    expect(nameErrorText(err("governance.contest_closed", { kind: "contestLabel", label: "a11ce" }))).toBe("Voting on “a11ce” has ended.");
    expect(nameErrorText(err("governance.contest_not_found", { kind: "contestLabel", label: "b0b" }))).toBe("Platform has no vote on “b0b”.");
    expect(nameErrorText(err("governance.not_a_contender", { kind: "contender", identity: ID_1 }))).toBe("8ZtW…R97P isn't a contender for this name.");
    expect(nameErrorText(err("governance.list_unavailable", null))).not.toBeNull();
    expect(nameErrorText(err("platform.unavailable", null))).toBe("Couldn't reach Dash Platform. Try again in a moment.");
    expect(nameErrorText(new Error("boom"))).toBeNull();
    // The vectors' codes carry the data the sentences use.
    for (const e of VECTORS.errors as AppError[]) {
      if (e.data?.kind === "contestLabel") expect(nameErrorText(e), e.code).toContain(`“${e.data.label}”`);
      if (e.data?.kind === "contender") expect(nameErrorText(e), e.code).toContain(shortIdentity(e.data.identity));
    }
  });
});
