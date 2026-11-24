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
import { DEFAULT_PREFS, ui } from "../app/ui";
import { FIXTURE_NAMES, ID_1, ID_2, VOTER_A, VOTER_B, VOTER_C } from "../fixture/names";
import { LANGUAGES, type Language } from "../i18n";
import { FIXTURE_NOW, secs } from "../fixture/wallet";
import {
  choiceOptions,
  choiceText,
  contenderText,
  contestRow,
  defaultVoters,
  endsText,
  isClosed,
  lastVoteNotice,
  listContests,
  nameErrorText,
  nameRejectedText,
  nameVoteResult,
  NameText,
  nameVoterRows,
  normalizeLabel,
  pickedVoters,
  sameChoice,
  shortIdentity,
  tallyView,
  voteBlocked,
  voteButtonText,
  voterLine,
  votesLeftText,
  votesText,
  youText,
} from "./names";

const NOW = secs(FIXTURE_NOW);
const VECTORS = JSON.parse(readFileSync(new URL("../fixture/vectors/governance.json", import.meta.url), "utf8"));
const names = (): ContestedNames => structuredClone(FIXTURE_NAMES);
const a11ce = (n = names()) => n.contests[0];
const A11CE = FIXTURE_NAMES.contests[0];
const DASH_PAY = FIXTURE_NAMES.contests[1];
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
    // F1: a contender whose own spelling came back reads by it, with the id; one without, by the id.
    expect(voteButtonText(towards(ID_1), A11CE)).toBe("Vote for Alice (8ZtW…R97P)");
    expect(voteButtonText(towards(ID_2), A11CE)).toBe("Vote for 4EfA…iSeF");
    expect(contenderText(ID_2, "Dash-Pay")).toBe("Dash-Pay (4EfA…iSeF)");
    expect(choiceText(towards(ID_2), DASH_PAY)).toBe("Dash-Pay (4EfA…iSeF)");
    expect(choiceText(towards(ID_1), DASH_PAY)).toBe("8ZtW…R97P");
    expect(contenderText(ID_1)).toBe("8ZtW…R97P");
    expect(voteButtonText(LOCK)).toBe("Vote Lock");
    expect(sameChoice(towards(ID_1), towards(ID_1))).toBe(true);
    expect(sameChoice(towards(ID_1), towards(ID_2))).toBe(false);
    expect(sameChoice(LOCK, LOCK)).toBe(true);
    expect(sameChoice(null, LOCK)).toBe(false);
    // The listed contenders, then Abstain and Lock, each with its votes.
    expect(choiceOptions(a11ce()).map((o) => [o.title, o.named, o.votes])).toEqual([
      ["Alice (8ZtW…R97P)", true, 9],
      ["4EfA…iSeF", false, 4],
      ["Abstain", false, 0],
      ["Lock", false, 5],
    ]);
  });

  it("words the votes left as Android does", () => {
    expect([0, 3, 4, 5, 7].map((used) => votesLeftText(used, 5))).toEqual(["5 votes left", "2 votes left", "One vote left", "No votes left", "No votes left"]);
  });
});

describe("the tally", () => {
  it("leads with the contender with most votes", () => {
    const t = tallyView(a11ce());
    expect(t.total).toBe(18);
    expect(t.rows.map((r) => [r.key, r.votes, r.leading])).toEqual([
      [`id:${ID_1}`, 9, true],
      [`id:${ID_2}`, 4, false],
      ["lock", 5, false],
      ["abstain", 0, false],
    ]);
    expect(t.rows[0].pct).toBeCloseTo((9 / 18) * 100);
    expect(t.lead).toBe("Alice (8ZtW…R97P) leads with 9 of 18 votes");
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
    expect(row).toMatchObject({ label: "a11ce", you: "You: 2 of 3 voted", total: 18, chip: null });
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
    expect(youText(n.voters, DASH_PAY)).toBe("You: 1 of 3 voted");
    expect(youText([n.voters[1]], A11CE)).toBe("You: Lock");
    expect(youText([n.voters[2]], A11CE)).toBe("You: no vote yet");
    expect(youText([n.voters[0]], A11CE)).toBe("You: Alice (8ZtW…R97P)");
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
    expect(voterLine(n.voters[1], A11CE, 5)).toBe("Votes Lock · No votes left");
    expect(voterLine(n.voters[2], A11CE, 5)).toBe("No vote yet · 5 votes left");
  });
});

describe("contenders by their own spelling (F1)", () => {
  /** a11ce with two look-alike spellings, each its own contender. */
  const lookAlike = (): ContestedName => ({
    ...structuredClone(A11CE),
    contenders: [
      { identity: ID_1, votes: 9, name: "Alice" },
      { identity: ID_2, votes: 4, name: "A11ce" },
    ],
  });

  it("tells look-alike spellings apart by the identity in every sentence", () => {
    const c = lookAlike();
    const n = names();
    n.voters[0].current[0].choice = towards(ID_2);
    expect(tallyView(c).rows.slice(0, 2).map((r) => r.title)).toEqual(["Alice (8ZtW…R97P)", "A11ce (4EfA…iSeF)"]);
    expect(tallyView(c).lead).toBe("Alice (8ZtW…R97P) leads with 9 of 18 votes");
    expect(choiceOptions(c).slice(0, 2).map((o) => o.title)).toEqual(["Alice (8ZtW…R97P)", "A11ce (4EfA…iSeF)"]);
    expect([voteButtonText(towards(ID_1), c), voteButtonText(towards(ID_2), c)]).toEqual(["Vote for Alice (8ZtW…R97P)", "Vote for A11ce (4EfA…iSeF)"]);
    expect(voterLine(n.voters[0], c, 5)).toBe("Votes A11ce (4EfA…iSeF) · 3 votes left");
    expect(youText([n.voters[0]], c)).toBe("You: A11ce (4EfA…iSeF)");
    expect(nameVoterRows(n.voters, c, towards(ID_2), 5)[0].sub).toBe("Votes A11ce (4EfA…iSeF) already");
    expect(nameVoteResult([cast(VOTER_C, null)], n.voters, c, towards(ID_1)).line).toBe("For Alice (8ZtW…R97P) on a11ce · weight 1 of 1");
    expect(nameVoteResult([cast(VOTER_C, null)], n.voters, c, towards(ID_2)).line).toBe("For A11ce (4EfA…iSeF) on a11ce · weight 1 of 1");
  });

  it("shows a contender without a spelling by its identity alone", () => {
    const c = { ...lookAlike(), contenders: [{ identity: ID_1, votes: 9, name: null }, { identity: ID_2, votes: 4, name: "" }] };
    expect(choiceOptions(c).slice(0, 2).map((o) => [o.title, o.named])).toEqual([
      ["8ZtW…R97P", false],
      ["4EfA…iSeF", false],
    ]);
    expect(tallyView(c).lead).toBe("8ZtW…R97P leads with 9 of 18 votes");
    expect(voteButtonText(towards(ID_1), c)).toBe("Vote for 8ZtW…R97P");
    expect(choiceText(towards(ID_1), c)).toBe("8ZtW…R97P");
    expect(contenderText(ID_2, "")).toBe("4EfA…iSeF");
  });

  it("keeps a spelling's exact characters, markup and placeholders included, as plain text", () => {
    for (const name of ["<b>Alice</b>", "<img src=x onerror=alert(1)>", "{identity}", "{name} {count}", "ALiCE", "a&amp;b"]) {
      const c = { ...lookAlike(), contenders: [{ identity: ID_1, votes: 9, name }] };
      expect(contenderText(ID_1, name)).toBe(`${name} (8ZtW…R97P)`);
      expect(choiceText(towards(ID_1), c)).toBe(`${name} (8ZtW…R97P)`);
      expect(tallyView(c).lead).toBe(`${name} (8ZtW…R97P) leads with 9 of 14 votes`);
      expect(voteButtonText(towards(ID_1), c)).toBe(`Vote for ${name} (8ZtW…R97P)`);
    }
  });
});

describe("the vote sheet's voters", () => {
  it("offers every voter with a vote left, a banned one too, and says whose key it is", () => {
    const n = names();
    const rows = nameVoterRows(n.voters, A11CE, null, n.votesAllowed);
    expect(rows.map((r) => [r.name, r.eligible, r.sub, r.present])).toEqual([
      ["Masternode a1a1a1a1", true, "key in this wallet · voting key 1 · 3 votes left", "Alice (8ZtW…R97P)"],
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
      const b = nameVoterRows(n.voters, A11CE, choice, n.votesAllowed).find((r) => r.proTxHash === VOTER_B)!;
      expect(b.eligible, JSON.stringify(choice)).toBe(false);
    }
    // At 4 of 5 it may still vote, and the sheet says it is its last.
    n.voters[0].current[0].votesUsed = 4;
    const rows = nameVoterRows(n.voters, A11CE, ABSTAIN, n.votesAllowed);
    expect(rows[0]).toMatchObject({ eligible: true, sub: "key in this wallet · voting key 1 · One vote left" });
    expect(lastVoteNotice(pickedVoters(rows, new Set([VOTER_A, VOTER_C])))).toBe("This is the last vote Masternode a1a1a1a1 has on this name: after it, its choice can't change.");
    expect(lastVoteNotice(pickedVoters(rows, new Set([VOTER_C])))).toBeNull();
  });

  it("turns off a voter whose present choice is the one picked (Platform's already_voted)", () => {
    const n = names();
    const rows = nameVoterRows(n.voters, A11CE, towards(ID_1), n.votesAllowed);
    expect(rows[0]).toMatchObject({ eligible: false, sub: "Votes Alice (8ZtW…R97P) already" });
    expect(rows[2].eligible).toBe(true);
  });

  it("names a voter by the wallet's label and service when it has one (MG-09 F3)", () => {
    const n = names();
    const known = new Map([
      [VOTER_A, { label: "Home node", service: "203.0.113.7:19999" }],
      [VOTER_B, { label: null, service: null }],
    ]);
    const rows = nameVoterRows(n.voters, A11CE, null, n.votesAllowed, known);
    expect(rows.map((r) => r.name)).toEqual(["Home node · 203.0.113.7:19999", "Evonode b2b2b2b2", "Masternode c3c3c3c3"]);
    n.voters[0].current[0].votesUsed = 4;
    const last = nameVoterRows(n.voters, A11CE, ABSTAIN, n.votesAllowed, known);
    expect(lastVoteNotice(pickedVoters(last, new Set([VOTER_A])))).toBe("This is the last vote Home node · 203.0.113.7:19999 has on this name: after it, its choice can't change.");
    const view = nameVoteResult([cast(VOTER_A, null), cast(VOTER_C, null)], n.voters, A11CE, ABSTAIN, undefined, known);
    expect(view.rows.map((r) => r.name)).toEqual(["Home node · 203.0.113.7:19999", "Masternode c3c3c3c3"]);
  });

  // Mutant 3 (an ineligible voter sent) dies here: a checked voter that can't vote is not picked.
  it("sends only checked voters that can vote with the choice", () => {
    const n = names();
    const all = new Set([VOTER_A, VOTER_B, VOTER_C]);
    expect(pickedVoters(nameVoterRows(n.voters, A11CE, towards(ID_1), n.votesAllowed), all).map((r) => r.proTxHash)).toEqual([VOTER_C]);
    expect(pickedVoters(nameVoterRows(n.voters, A11CE, ABSTAIN, n.votesAllowed), new Set([VOTER_B, VOTER_C])).map((r) => r.proTxHash)).toEqual([VOTER_C]);
    expect(pickedVoters(nameVoterRows(n.voters, DASH_PAY, LOCK, n.votesAllowed), all).map((r) => r.proTxHash)).toEqual([VOTER_A, VOTER_B, VOTER_C]);
  });
});

describe("the result", () => {
  it("counts the counted votes with their weight", () => {
    const n = names();
    expect(nameVoteResult([cast(VOTER_C, null)], n.voters, A11CE, towards(ID_1)).line).toBe("For Alice (8ZtW…R97P) on a11ce · weight 1 of 1");
    const view = nameVoteResult([cast(VOTER_A, null), cast(VOTER_C, null)], n.voters, A11CE, towards(ID_2));
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
    const view = nameVoteResult(casts, n.voters, A11CE, ABSTAIN);
    expect(view.title).toBe("1 of 3 votes counted · 1 not confirmed yet");
    expect(view.line).toBe("Abstain on a11ce · weight 1 of 6");
    expect(view.rows[0]).toMatchObject({ state: "unconfirmed", sub: "Sent, but Platform didn't confirm it. It may still count: check again in a minute.", chip: { text: "Unconfirmed", tone: "neutral" } });
    expect(view.rows[1]).toMatchObject({ state: "refused", sub: "It has used all its votes on this name, so its choice can't change.", chip: { text: "Not counted" } });
    expect(view.unconfirmed).toBe(true);
    // A re-read that still shows the old choice keeps it unconfirmed.
    expect(nameVoteResult(casts, n.voters, A11CE, ABSTAIN, names()).rows[0].state).toBe("unconfirmed");
    // One that shows the cast choice counts it.
    const after = names();
    after.voters[0].current[0].choice = ABSTAIN;
    const checked = nameVoteResult(casts, n.voters, A11CE, ABSTAIN, after);
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
    // A reason dw-app doesn't know says so, apart from Platform's `refused`, and isn't counted.
    const unknown = nameVoteResult([cast(VOTER_A, "unknown")], names().voters, A11CE, LOCK);
    expect(unknown.rows[0]).toMatchObject({ state: "refused", sub: "Not counted, for a reason this version of the wallet doesn't know." });
    expect(nameRejectedText("unknown")).not.toBe(nameRejectedText("refused"));
    // An unknown masternode in the result is named by its hash.
    const view = nameVoteResult([cast("d4".repeat(32), "unknown_masternode")], names().voters, A11CE, LOCK);
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

describe("counts and weights", () => {
  const big = (): ContestedNames => {
    const n = names();
    n.contests[0].contenders[0].votes = 1234;
    n.voters[1].weight = 1234;
    return n;
  };
  const read = () => {
    const n = big();
    const result = nameVoteResult([cast(VOTER_B, null)], n.voters, A11CE, LOCK);
    return [votesText(1234), tallyView(a11ce(n)).lead, result.title, result.line, result.rows[0].sub, NameText.limitNote(5), NameText.limitNote(1)];
  };

  it("read as before in English, with counts and weights of 1,000 or more grouped", () => {
    expect(read()).toEqual([
      "1,234 votes",
      "Alice (8ZtW…R97P) leads with 1,234 of 1,243 votes",
      "1 of 1 vote counted",
      "Lock on a11ce · weight 1,234 of 1,234",
      "weight 1,234 · Platform counted it",
      "Each masternode has 5 votes per username: its first vote and 4 changes. This wallet counts the votes it sent, so votes sent from another wallet aren't included.",
      "Each masternode has 1 votes per username: its first vote and 0 changes. This wallet counts the votes it sent, so votes sent from another wallet aren't included.",
    ]);
  });

  it("are grouped for the display language", () => {
    const de: Language = { tag: "de-x-dwtest", name: "Deutsch", dir: "ltr", messages: new Map() };
    LANGUAGES.push(de);
    try {
      ui.set({ prefs: { ...DEFAULT_PREFS, language: "de-x-dwtest" } });
      expect(read().slice(0, 5)).toEqual(["1.234 votes", "Alice (8ZtW…R97P) leads with 1.234 of 1.243 votes", "1 of 1 vote counted", "Lock on a11ce · weight 1.234 of 1.234", "weight 1.234 · Platform counted it"]);
    } finally {
      LANGUAGES.pop();
      ui.set({ prefs: { ...DEFAULT_PREFS, language: null } });
    }
  });

  it("an unknown rejection reason never reads an inherited property", () => {
    expect(nameRejectedText("toString" as never)).toBe("Not counted, for a reason this version of the wallet doesn't know.");
  });
});
