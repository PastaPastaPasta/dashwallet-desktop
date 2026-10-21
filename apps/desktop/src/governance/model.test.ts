// The Governance tab's view model (model.ts, MG-09): the clock and budget cards, chips and margins,
// the wallet's votes and the vote sheet's rows and result, error sentences, Mine's states per
// MG-06 D24, the sidebar rule and the create wizard's helpers.

import { describe, expect, it } from "vitest";
import type { GovernanceInfo } from "../api/generated/GovernanceInfo";
import type { MyProposal } from "../api/generated/MyProposal";
import type { MyProposalState } from "../api/generated/MyProposalState";
import type { ProposalRow } from "../api/generated/ProposalRow";
import type { SuperblockDate } from "../api/generated/SuperblockDate";
import type { VotingMasternode } from "../api/generated/VotingMasternode";
import { FIXTURE_PROPOSALS, governanceInfo, syncedGovernance, newGovernanceWorld } from "../fixture/governance";
import { FIXTURE_NOW, secs } from "../fixture/wallet";
import {
  ago,
  askSpan,
  badgeCount,
  budgetNotice,
  budgetView,
  clockView,
  createdFrom,
  createdToast,
  defaultFirstPayment,
  detailsProblems,
  feeGrantCap,
  feeTotal,
  FIELD_STEP,
  fieldText,
  governanceErrorText,
  inProgress,
  lastPaymentText,
  lastVoteLine,
  linkText,
  listState,
  marginText,
  masternodesShown,
  mineRow,
  needsMyVote,
  netRange,
  nodeName,
  numbersRows,
  paneNote,
  presetOutcome,
  rejectedText,
  sameFee,
  selectionNote,
  shortAddress,
  sortRows,
  spanLong,
  spanShort,
  statusChip,
  voteBar,
  voteResult,
  voteRows,
  votesView,
  youSummary,
} from "./model";

const L = "en-US";
const NOW = secs(FIXTURE_NOW);
const COIN = 100_000_000;
const INFO = governanceInfo(syncedGovernance());
const ROWS = FIXTURE_PROPOSALS.map((p) => p.row);
const row = (name: string) => ROWS.find((r) => r.name === name)!;

const node = (patch: Partial<VotingMasternode> = {}): VotingMasternode => ({
  proTxHash: "1a2b3c4d".repeat(8),
  kind: "regular",
  weight: 1,
  valid: true,
  keyIndex: 0,
  currentVote: null,
  voteTime: null,
  keyHolders: 1,
  keyLastVote: null,
  keyLastVoteTime: null,
  ...patch,
});

describe("times", () => {
  it("spans the long and short way, two parts at most", () => {
    expect(spanLong(86_400 + 6 * 3600 + 59)).toBe("1 day 6 hours");
    expect(spanLong(2 * 86_400)).toBe("2 days");
    expect(spanLong(3600 + 120)).toBe("1 hour 2 minutes");
    expect(spanLong(10)).toBe("1 minute");
    expect(spanShort(86_400 + 6 * 3600)).toBe("1 d 6 h");
    expect(spanShort(40 * 60)).toBe("40 min");
    expect(ago(NOW - 30, NOW)).toBe("just now");
    expect(ago(NOW - 40 * 60, NOW)).toBe("40 min ago");
    expect(ago(NOW - 2 * 3600, NOW)).toBe("2 hours ago");
  });
});

describe("the clock card", () => {
  it("counts down to the estimated cutoff while voting is open", () => {
    const c = clockView(INFO, NOW, L);
    expect(c.title).toBe("About 1 day 7 hours left to vote for the Oct 10 payout");
    expect(c.sub).toMatch(/^Next payout to proposals: Oct 10, around .+ · block 2,375,048\. Later votes may only count for later payouts\.$/);
    expect(c.statusBar).toBe("Oct 10 votes: about 1 d 7 h left");
    expect(c.labels[1]).toBe("estimated cutoff, Oct 7");
    expect(c.labels[0]).toMatch(/ · last payout$/);
    // The tip and the cutoff on the cycle's bar.
    expect(c.done).toBeCloseTo(((2_372_666 - (2_375_048 - 16_616)) / 16_616) * 100, 5);
    expect(c.cutoff).toBeCloseTo(((2_373_386 - (2_375_048 - 16_616)) / 16_616) * 100, 5);
  });

  it("says when votes may be too late, and when the payout is due", () => {
    const closed: GovernanceInfo = { ...INFO, clock: { ...INFO.clock, votingOpen: false } };
    expect(clockView(closed, NOW, L)).toMatchObject({ title: "Votes now may be too late for the Oct 10 payout", statusBar: null });
    const due: GovernanceInfo = { ...INFO, clock: { ...INFO.clock, blocksToSuperblock: 0 } };
    expect(clockView(due, NOW, L)).toMatchObject({ title: "Payout due now (block 2,375,048)", statusBar: null });
    const first: GovernanceInfo = { ...INFO, clock: { ...INFO.clock, lastSuperblock: 0 } };
    expect(clockView(first, NOW, L).labels[0]).toBe("cycle start");
  });
});

describe("the budget card", () => {
  it("shows the exact budget before sync and the estimate after", () => {
    const before = governanceInfo(newGovernanceWorld());
    expect(budgetView(before, undefined, L)).toEqual({ title: "Budget for Oct 10", synced: null, budget: 6_018 * COIN });
    const v = budgetView(INFO, ROWS, L);
    expect(v.synced).toMatchObject({ allocated: 1_500 * COIN, passing: 1, overCount: 1, overAmount: 220 * COIN });
    expect(v.synced!.passingPct).toBeCloseTo((1_500 / 6_018) * 100, 5);
  });
});

describe("the list", () => {
  it("maps each sync phase to its state", () => {
    const s = syncedGovernance().sync;
    expect(listState({ ...s, phase: "disabled" })).toEqual({ kind: "disabled" });
    expect(listState({ ...s, phase: "failed" }).kind).toBe("failed");
    expect(listState(s)).toEqual({ kind: "ready" });
    expect(listState({ ...s, phase: "syncingVotes", objects: 1_204, votes: 12_345 })).toMatchObject({
      kind: "syncing",
      line: "1,204 proposals and 12,345 votes so far. Counts appear when the sync is complete, never partly.",
    });
    expect(listState({ ...s, phase: "waiting" }).kind).toBe("syncing");
  });

  it("chips the status, with Too close to call over the vote-derived ones (D10)", () => {
    expect(statusChip("passing").text).toBe("Passing");
    expect(statusChip("unfunded").tone).toBe("warning");
    expect(statusChip("passing", true).text).toBe("Too close to call");
    expect(statusChip("funded", true).text).toBe("Funded");
    expect(statusChip("lapsed").text).toBe("Ended");
  });

  it("states the margin to the funding line, with the range on a close call", () => {
    expect(marginText(row("dash-incubator-nov"), 357)).toBe("about +414 over");
    expect(marginText(row("dashcon-2027-venue"), 357)).toBe("about 692 short");
    expect(marginText(row("alt-wallet-hardening"), 357)).toBe("about 6 short; −18 to +6");
    expect(marginText(row("old-website-redesign"), 357)).toBeNull();
    expect(netRange(row("alt-wallet-hardening").tally)).toEqual([339, 363]);
  });

  it("draws the vote bar in percent of the weight", () => {
    const b = voteBar(row("dash-incubator-nov").tally, 357, 3_570);
    expect(b.line).toBeCloseTo(10, 5);
    expect(b.yes).toBeCloseTo((812 / 3_570) * 100, 5);
    expect(voteBar(row("dash-incubator-nov").tally, 357, 0)).toEqual({ yes: 0, no: 0, abstain: 0, line: 0 });
  });

  it("sorts by the engine's order, size, start and name", () => {
    expect(sortRows(ROWS, "closest")).toEqual(ROWS);
    expect(sortRows(ROWS, "mostAsked")[0].name).toBe("alt-wallet-hardening");
    expect(sortRows(ROWS, "newest")[0].name).toBe("alt-wallet-hardening");
    expect(sortRows(ROWS, "name").map((r) => r.name)[0]).toBe("alt-wallet-hardening");
  });

  it("writes the pane's note, ask, votes and short forms", () => {
    expect(paneNote(row("dash-incubator-nov"), INFO, L)).toBe("paid Oct 10 if it stays above the line");
    expect(paneNote(row("dashcon-2027-venue"), INFO, L)).toBe("needs about 692 more net yes votes");
    expect(paneNote(row("alt-wallet-hardening"), INFO, L)).toBe("within the margin of the votes seen");
    expect(askSpan({ paymentsRequested: 1, startEpoch: NOW, endEpoch: NOW + 86_400 }, L)).toBe("once, Oct 6 to Oct 7");
    const v = votesView(row("dash-incubator-nov").tally, 357);
    expect(v.head).toBe("about 771 net yes · needs 357");
    expect(v.range).toBe("From the votes peers sent: 759 to 783 net yes. Unseen votes can change it.");
    expect(votesView(row("dashcon-2027-venue").tally, 357).range).toBe("From the votes peers sent. Unseen votes can change it.");
    expect(shortAddress("yQ1111Gov8vK2pMxRt5nW9cJ3hL6dF4sA1")).toBe("yQ11…4sA1");
    expect(linkText("https://www.dashcentral.org/p/x")).toBe("dashcentral.org/p/x");
  });
});

describe("the wallet's votes", () => {
  it("summarises the votes seen, a shared key's as the key's (D8)", () => {
    expect(youSummary(undefined)).toBeNull();
    expect(youSummary([node({ valid: false })])).toBeNull();
    expect(youSummary([node()])).toBe("You: no vote seen");
    expect(youSummary([node({ currentVote: "no" })])).toBe("You: No (last seen)");
    expect(youSummary([node({ currentVote: "yes" }), node(), node({ valid: false })])).toBe("You: 1 of 2 seen voting");
    expect(youSummary([node({ keyHolders: 2, keyLastVote: "yes" })])).toBe("You: Yes (last seen)");
    expect(lastVoteLine(node({ keyHolders: 2, keyLastVote: "yes" }), NOW)).toBe("Last vote seen with this shared key: Yes (2 masternodes use this key)");
    expect(lastVoteLine(node({ currentVote: "abstain", voteTime: NOW - 40 * 60 }), NOW)).toBe("Abstain · last vote seen 40 min ago");
    expect(nodeName(node({ kind: "evonode" }))).toBe("Evonode 1a2b3c4d");
  });

  it("needs my vote: open, with a valid masternode and no vote seen", () => {
    expect(needsMyVote(row("dashcon-2027-venue"), [node()])).toBe(true);
    expect(needsMyVote(row("dashcon-2027-venue"), [node({ currentVote: "yes" })])).toBe(false);
    expect(needsMyVote(row("dashcon-2027-venue"), [node({ valid: false })])).toBe(false);
    expect(needsMyVote(row("old-website-redesign"), [node()])).toBe(false);
    expect(needsMyVote(row("dashcon-2027-venue"), undefined)).toBe(false);
  });
});

describe("the vote sheet", () => {
  it("disables a banned masternode and one that voted within the hour (D11)", () => {
    const rows = voteRows(
      [
        node({ proTxHash: "a".repeat(64) }),
        node({ proTxHash: "b".repeat(64), valid: false }),
        node({ proTxHash: "c".repeat(64), currentVote: "yes", voteTime: NOW - 40 * 60 }),
        node({ proTxHash: "d".repeat(64), kind: "evonode", weight: 4, keyIndex: 2, keyHolders: 3 }),
      ],
      NOW,
      L,
    );
    expect(rows.map((r) => r.eligible)).toEqual([true, false, false, true]);
    expect(rows[1].sub).toBe("PoSe banned: a banned masternode can't vote");
    expect(rows[2].sub).toMatch(/^Voted Yes 40 min ago · can change after \d{1,2}:20 PM$/);
    expect(rows[3].sub).toBe("key in this wallet · voting key 3 · 3 masternodes use this key");
    expect(selectionNote([rows[0], rows[3]])).toBe("2 masternodes · weight 5 · no fee");
    expect(selectionNote([rows[0]])).toBe("1 masternode · weight 1 · no fee");
  });

  it("presets the outcome only when every valid masternode saw the same one", () => {
    expect(presetOutcome([node({ currentVote: "no" }), node({ currentVote: "no" }), node({ valid: false })])).toBe("no");
    expect(presetOutcome([node({ currentVote: "no" }), node()])).toBeNull();
    expect(presetOutcome([node()])).toBeNull();
  });

  it("lists each cast: sent with its peers, or the refusal as a sentence (D12, D13)", () => {
    const a = node({ proTxHash: "a".repeat(64), weight: 4, kind: "evonode" });
    const b = node({ proTxHash: "b".repeat(64) });
    const v = voteResult(
      [
        { proTxHash: a.proTxHash, hash: "f".repeat(64), peers: 8, rejected: null },
        { proTxHash: b.proTxHash, hash: null, peers: 0, rejected: "wrong_voting_key" },
      ],
      [a, b],
      "yes",
    );
    expect(v).toMatchObject({ title: "1 of 2 votes sent", line: "Yes · weight 4 of 5" });
    expect(v.rows[0].sub).toBe("weight 4 · to 8 peers");
    expect(v.rows[1].sub).toBe("Its voting key changed, so the key in this wallet no longer matches.");
    expect(voteResult([{ proTxHash: a.proTxHash, hash: "f".repeat(64), peers: 0, rejected: null }], [a], "no").rows[0].sub).toBe(
      "weight 4 · sent when the wallet reconnects",
    );
    expect(rejectedText("something_new")).toBe("The network refused the vote.");
  });
});

describe("error sentences", () => {
  const err = (code: string, data: unknown = null) => ({ code, message: code, data });
  it("maps every governance code, and leaves the rest to the send sentences", () => {
    expect(governanceErrorText(err("governance.vote_rejected", { kind: "reason", reason: "too_often" }))).toBe("The network accepts one change per masternode per hour.");
    expect(governanceErrorText(err("governance.submit_refused", { kind: "reason", reason: "in_flight" }))).toBe("The fee is being sent. Try again in a moment.");
    expect(governanceErrorText(err("governance.submit_refused", { kind: "reason", reason: "?" }))).toBe("This proposal can't be submitted now.");
    for (const code of ["sync_disabled", "not_synced", "proposal_not_found", "no_peers", "list_unavailable", "invalid_proposal", "not_implemented"])
      expect(governanceErrorText(err(`governance.${code}`)), code).toMatch(/\.$/);
    expect(governanceErrorText(err("send.grant_exceeded"))).toBeNull();
    expect(governanceErrorText(new Error("x"))).toBeNull();
  });
});

describe("Mine (MG-06 D24, D15–D17)", () => {
  const mine = (state: MyProposalState, lastError: string | null = null): MyProposal => ({
    hash: "e".repeat(64),
    name: "p",
    url: "https://x.org/p",
    paymentAddress: "yPay8vK2pMxRt5nW9cJ3hL6dF4sA1bZ7Wm",
    paymentAmount: COIN,
    paymentCount: 1,
    startEpoch: NOW,
    endEpoch: NOW + 86_400,
    feeTxid: "f".repeat(64),
    createdAt: NOW,
    submittedAt: null,
    state,
    lastError,
  });
  it("shows Submit exactly where the engine acts on it", () => {
    const cases: [MyProposalState, boolean][] = [
      [{ kind: "feeUnsent" }, true],
      [{ kind: "feePaid", confirmations: 0 }, true],
      [{ kind: "feePaid", confirmations: 1 }, false],
      [{ kind: "readyToSubmit", reason: "noPeerAccepted" }, true],
      [{ kind: "readyToSubmit", reason: "notSynced" }, false],
      [{ kind: "readyToSubmit", reason: "syncDisabled" }, false],
      [{ kind: "confirming", confirmations: 3 }, false],
      [{ kind: "public", status: "passing" }, false],
      [{ kind: "feeFailed" }, false],
      [{ kind: "expired" }, false],
    ];
    for (const [state, submit] of cases) expect(mineRow(mine(state), L).submit, JSON.stringify(state)).toBe(submit);
  });

  it("names each state, the failure behind Submit and Turn on", () => {
    expect(mineRow(mine({ kind: "feeUnsent" }, "send: broadcast outcome unknown: x"), L)).toMatchObject({
      chip: { text: "Fee not sent" },
      error: "The fee may not have reached the network. Submit sends it again.",
      attention: true,
    });
    // Any kept failure may have left the fee out: Submit, never a new proposal (D35).
    expect(mineRow(mine({ kind: "feeUnsent" }, "send: no_peers"), L).error).toBe("The fee may not have reached the network (no_peers). Submit sends the same fee again.");
    expect(mineRow(mine({ kind: "feeUnsent" }), L).attention).toBe(false);
    expect(mineRow(mine({ kind: "feePaid", confirmations: 0 }), L)).toMatchObject({ chip: { text: "Fee paid" }, meta: "0 of 1" });
    expect(mineRow(mine({ kind: "confirming", confirmations: 9 }), L).meta).toBe("6 of 6");
    expect(mineRow(mine({ kind: "readyToSubmit", reason: "syncDisabled" }), L).turnOn).toBe(true);
    expect(mineRow(mine({ kind: "readyToSubmit", reason: "noPeerAccepted" }), L).attention).toBe(true);
    expect(mineRow(mine({ kind: "public", status: null }), L).chip.text).toBe("Submitted");
    expect(mineRow(mine({ kind: "public", status: "unfunded" }), L).chip.text).toBe("Over budget");
    expect(mineRow(mine({ kind: "feeFailed" }), L).chip.tone).toBe("danger");
    expect(mineRow(mine({ kind: "expired" }), L).chip.text).toBe("Ended");
    expect(createdToast(mine({ kind: "feePaid", confirmations: 0 }))).toMatch(/: fee paid\. Mine shows its progress\.$/);
    expect(createdToast(mine({ kind: "feeUnsent" }, "send: no_peers"))).toMatch(/: created, but its fee didn't go out\. Mine shows why\.$/);
    expect(badgeCount([mine({ kind: "feeUnsent" }, "send: x"), mine({ kind: "readyToSubmit", reason: "noPeerAccepted" }), mine({ kind: "feePaid", confirmations: 0 })], L)).toBe(2);
  });

  it("shows the sidebar item for the setting, a proposal in progress or a collateral-sized coin (D3)", () => {
    expect(masternodesShown(true, undefined, undefined)).toBe(true);
    expect(masternodesShown(false, [mine({ kind: "confirming", confirmations: 1 })], undefined)).toBe(true);
    expect(masternodesShown(false, [mine({ kind: "expired" }), mine({ kind: "feeFailed" })], [5 * COIN])).toBe(false);
    expect(masternodesShown(false, [], [1_000 * COIN])).toBe(true);
    expect(masternodesShown(false, [], [4_000 * COIN])).toBe(true);
    expect(masternodesShown(false, [], [1_000 * COIN + 1])).toBe(false);
    expect(inProgress(mine({ kind: "public", status: null }))).toBe(true);
  });
});

describe("the fee (D29, D33, D34)", () => {
  const preview = { fee: COIN, networkFee: 227 };
  it("totals the burn and the network fee, and caps the grant at that total and the margin, no more", () => {
    expect(feeTotal(preview)).toBe(100_000_227);
    // The margin is dwcli's: at least 10,000 duffs, else the network fee again.
    expect(feeGrantCap(preview)).toBe(100_000_227 + 10_000);
    expect(feeGrantCap({ fee: COIN, networkFee: 25_000 })).toBe(COIN + 50_000);
    for (const nf of [0, 1, 227, 9_999, 10_000, 10_001, 1_000_000]) {
      const p = { fee: COIN, networkFee: nf };
      expect(feeGrantCap(p)).toBeGreaterThanOrEqual(feeTotal(p) + 10_000);
      expect(feeGrantCap(p)).toBeLessThanOrEqual(feeTotal(p) + Math.max(nf, 10_000));
    }
  });

  it("asks a fresh confirm when Pay's re-read differs from the review in either figure", () => {
    expect(sameFee(preview, { ...preview })).toBe(true);
    expect(sameFee(preview, { ...preview, networkFee: 228 })).toBe(false);
    expect(sameFee(preview, { ...preview, fee: COIN + 1 })).toBe(false);
  });

  it("finds the proposal a failed create_proposal made from the draft", () => {
    const made: MyProposal = {
      hash: "e".repeat(64),
      name: "p",
      url: "https://x.org/p",
      paymentAddress: "yPay8vK2pMxRt5nW9cJ3hL6dF4sA1bZ7Wm",
      paymentAmount: COIN,
      paymentCount: 2,
      startEpoch: NOW,
      endEpoch: NOW + 86_400,
      feeTxid: "f".repeat(64),
      createdAt: NOW,
      submittedAt: null,
      state: { kind: "feeUnsent" },
      lastError: null,
    };
    const draft = { name: "p", url: "https://x.org/p", paymentAddress: made.paymentAddress, paymentAmount: COIN, paymentCount: 2, firstSuperblockHeight: 1, coins: [] };
    expect(createdFrom([made], draft)).toBe(made);
    expect(createdFrom([{ ...made, state: { kind: "feePaid", confirmations: 3 } }], draft)).toBeDefined();
    expect(createdFrom([made], { ...draft, paymentAmount: COIN + 1 })).toBeUndefined();
    expect(createdFrom([{ ...made, state: { kind: "expired" } }], draft)).toBeUndefined();
    expect(createdFrom([{ ...made, state: { kind: "feeFailed" } }], draft)).toBeUndefined();
    // One Mine held before Pay is an older proposal, not this call's.
    expect(createdFrom([made], draft, new Set([made.hash]))).toBeUndefined();
  });
});

describe("Create proposal", () => {
  const sb = (i: number, cutoffSoon = false): SuperblockDate => ({
    height: 2_375_048 + i * 16_616,
    estimatedTime: NOW + 4 * 86_400 + i * 30 * 86_400,
    budget: 6_018 * COIN,
    votingCutoffHeight: 2_373_386 + i * 16_616,
    votingCutoffEstimate: NOW + 86_400 + i * 30 * 86_400,
    cutoffSoon,
  });
  it("starts at the next superblock, or the one after when its cutoff is near (MG-06 D13)", () => {
    expect(defaultFirstPayment([], NOW)).toEqual({ height: null, notice: null });
    expect(defaultFirstPayment([sb(0), sb(1)], NOW)).toEqual({ height: sb(0).height, notice: null });
    const near = defaultFirstPayment([sb(0, true), sb(1)], NOW, L);
    expect(near.height).toBe(sb(1).height);
    expect(near.notice).toBe(
      "The estimated cutoff for the Oct 10 payout is in about 1 day, so the first payment is Nov 9. New proposals rarely gather enough votes in a day.",
    );
  });

  it("says the last payment, past the listed superblocks too", () => {
    const list = [sb(0), sb(1), sb(2)];
    expect(lastPaymentText(list, sb(0).height, 1, L)).toBe("One payment.");
    expect(lastPaymentText(list, sb(0).height, 3, L)).toBe("Last payment Dec 9, 2026.");
    expect(lastPaymentText(list, sb(1).height, 4, L)).toBe("Last payment about Feb 7, 2027.");
    expect(lastPaymentText(list, 1, 2, L)).toBeNull();
  });

  it("checks step 1 locally and maps the engine's fields to steps and messages", () => {
    expect(detailsProblems("desktop-wallet-i18n", "https://x.org")).toEqual({});
    expect(Object.keys(detailsProblems("Bad Name", ""))).toEqual(["name", "url"]);
    expect(FIELD_STEP.payment_address).toBe(2);
    expect(FIELD_STEP.payload).toBe(1);
    expect(fieldText("payment_address", "testnet")).toBe("Enter a Dash address on testnet.");
    expect(budgetNotice(governanceInfo(newGovernanceWorld()), String)).toBeNull();
    expect(budgetNotice(INFO, (d) => `${d / COIN} DASH`)).toMatch(/^Up to 6018 DASH is available at each superblock\. .+ about 1500 DASH, /);
  });
});

describe("Governance numbers", () => {
  it("states the cycle and the votes, the estimates after sync only", () => {
    const amount = (d: number) => `${d / COIN} DASH`;
    const { cycle, votes } = numbersRows(INFO, syncedGovernance().sync, ROWS, amount, L);
    expect(cycle.map((r) => r.value)).toEqual([
      "Block 2,358,432",
      "Block 2,373,386 · about Oct 7: votes after it may be too late for the Oct 10 payout",
      "Block 2,375,048 · about Oct 10",
      "16,616 blocks; the network starts fixing a payout 1,662 blocks before its superblock",
    ]);
    expect(votes[0].value).toBe("3,570: 2,850 masternodes and 180 evonodes, which count 4 each");
    expect(votes[3].value).toBe("about 1500 DASH to about 1 of 5 open · about 1 more over budget");
    expect(votes[4].value).toBe("Block 2,372,666 · 8 peers; some votes may be missing");
    const before = numbersRows(governanceInfo(newGovernanceWorld()), undefined, undefined, amount, L);
    expect(before.votes[3].value).toBe("after sync");
  });
});

// The row type is the engine's; keep the fixture's sample rows honest about it.
const _rows: ProposalRow[] = ROWS;
void _rows;
