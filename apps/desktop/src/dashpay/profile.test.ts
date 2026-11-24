// My Profile's view model (DP1-10), by parity ID: IOS-069 (the profile sheet: names, credits and
// the low-credit warnings; Android's AND-078 estimate), IOS-068 (the username request's tallies,
// deadline and temporary name, the contest outcomes; Android's AND-071).

import { describe, expect, it } from "vitest";
import type { ContestStatus } from "../api/generated/ContestStatus";
import type { CostTable } from "../api/generated/CostTable";
import type { DashPayEventInfo } from "../api/generated/DashPayEventInfo";
import type { IdentitySummary } from "../api/generated/IdentitySummary";
import type { PendingTopUp } from "../api/generated/PendingTopUp";
import { creditBalance, creditsModel, estimateText, headerModel, holdsTopUp, outcomeRows, pageModel, PROFILE_COPY, requestModel, tallyRows, type CreditsInputs } from "./profile";
import { homeCard, joinModel, type StatusInputs } from "./status";
import { outcomeText } from "./topup";
import { INSTANT_COPY, registrationOffered, VOTE_COPY, type InstantInputs } from "./instantOffer";

const COSTS: CostTable = {
  protocolVersion: 10,
  contactRequest: "94048500",
  profileUpdate: "120000000",
  contactInfo: "110000000",
  enableDashpayKeys: "61330100",
  creditsPerDuff: 1000,
  topUpFee: "50000000",
  topUpMinDuffs: 50_000,
  lowCredits: "1200000000",
  emptyCredits: "61330100",
};

const alice = (patch: Partial<IdentitySummary> = {}): IdentitySummary => ({
  identity: "AliceId",
  index: 0,
  names: ["alice"],
  mainName: "alice",
  namesUpdating: false,
  namesCheck: null,
  isMain: true,
  balance: "184500000000",
  balanceAsOf: 0,
  balanceStale: false,
  hasDashpayKeys: true,
  profile: { displayName: "Alice Liddell", publicMessage: "Down the rabbit hole", avatarUrl: null, avatarHash: null, avatarFingerprint: null, updatedAt: null },
  unverified: false,
  creditLevel: "ok",
  ...patch,
});

const amount = (duffs: number) => `${duffs / 1e8} tDASH`;

const credits = (rest: Partial<CreditsInputs> = {}): CreditsInputs => ({ costs: COSTS, pending: [], stuck: [], toppingUp: false, unseen: false, answeredUnknown: new Set(), amount, masked: "***", discreet: false, ...rest });

const statusInputs = (rest: Partial<StatusInputs>): StatusInputs => ({
  status: { kind: "ready", main: "AliceId" },
  statusError: false,
  main: alice(),
  registration: undefined,
  lock: "unlocked",
  syncPercent: null,
  date: (secs) => `day ${secs}`,
  amount,
  discreet: false,
  ...rest,
});

const lock = (patch: Partial<PendingTopUp> = {}): PendingTopUp => ({ txid: "aa", identity: "AliceId", identityIndex: 0, duffs: 5_000_000, proved: false, consumptionUnknown: false, ...patch });

describe("the page (IOS-069)", () => {
  it("P1–P6: loading, the status card, no profile, the identity's page", () => {
    expect(pageModel(statusInputs({ status: undefined, main: undefined })).kind).toBe("loading");
    expect(pageModel(statusInputs({ main: undefined })).kind).toBe("loading");
    expect(pageModel(statusInputs({ statusError: true }))).toMatchObject({ kind: "card", noProfile: false, card: { state: "error" } });
    expect(pageModel(statusInputs({ status: { kind: "noIdentity", reason: null }, main: undefined }))).toMatchObject({ kind: "card", noProfile: true, card: { state: "join" } });
    expect(pageModel(statusInputs({}))).toMatchObject({ kind: "ready", contest: null });
    expect(pageModel(statusInputs({ status: { kind: "contestPending", identity: "AliceId", label: "al", endsAt: 9 } }))).toMatchObject({ kind: "ready", contest: { label: "al", endsAt: 9 } });
  });

  it("D4: the header's display name, @name, message and initials; the names states", () => {
    expect(headerModel(alice())).toEqual({ title: "Alice Liddell", username: "@alice", names: "named", message: "Down the rabbit hole", initials: "A", shareable: true });
    expect(headerModel(alice({ profile: null }))).toMatchObject({ title: "alice", message: null, initials: "A" });
    expect(headerModel(alice({ profile: null, mainName: null, names: [] }))).toMatchObject({ title: PROFILE_COPY.noUsername, username: PROFILE_COPY.noUsername, initials: "?" });
    expect(headerModel(alice({ profile: null, mainName: null, names: [] }), "al").username).toBe("@al · Pending");
    expect(headerModel(alice({ mainName: null, namesCheck: "failed" })).username).toBe(PROFILE_COPY.notLoaded);
    expect(headerModel(alice({ mainName: null, namesUpdating: true })).username).toBe(PROFILE_COPY.updating);
    // A name being refreshed or rechecked has no link yet (`platform.unavailable`), so no Share.
    expect(headerModel(alice({ namesUpdating: true }))).toMatchObject({ names: "named", shareable: false });
    expect(headerModel(alice({ namesCheck: "pending" }))).toMatchObject({ names: "named", shareable: false });
    expect(headerModel(alice({ mainName: null, names: [] })).shareable).toBe(false);
  });
});

describe("credits (IOS-069, AND-078)", () => {
  it("D6: the balance in Dash and Android's action estimate", () => {
    expect(estimateText("184500000000", COSTS)).toBe("≈ 1,961 contact requests · 1,537 profile updates");
    const m = creditsModel(alice(), credits());
    expect(m).toMatchObject({ level: "ok", figure: "1.845 tDASH", updating: false, warning: null, topUp: { disabled: false, primary: false } });
    expect(m.estimate).toBe("≈ 1,961 contact requests · 1,537 profile updates");
  });

  it("C2, C3 (D8): the low and empty warnings make Top up the primary action", () => {
    expect(creditsModel(alice({ creditLevel: "low", balance: "1000000000" }), credits())).toMatchObject({ warning: PROFILE_COPY.credits.low, topUp: { primary: true } });
    expect(creditsModel(alice({ creditLevel: "empty", balance: "4000000" }), credits())).toMatchObject({ warning: PROFILE_COPY.credits.empty, topUp: { primary: true } });
  });

  it("C4, C5 (D9): an unknown balance, a stale one", () => {
    expect(creditsModel(alice({ balance: null, creditLevel: null, balanceStale: true }), credits())).toMatchObject({ level: "unknown", figure: null, estimate: null, updating: false });
    expect(creditsModel(alice({ balanceStale: true }), credits()).updating).toBe(true);
  });

  it("a known balance before the cost table is read shows its credits (review m3)", () => {
    expect(creditsModel(alice({ balance: "1845000000" }), credits({ costs: undefined }))).toMatchObject({ figure: "1,845,000,000 credits", estimate: null });
    expect(creditsModel(alice(), credits({ costs: undefined, discreet: true })).figure).toBe("***");
  });

  it("D30: the top-up sheet's balance before the cost table is read shows its credits; only Discreet Mode masks", () => {
    const amount = (duffs: number) => `${duffs} duffs`;
    const applied = { txid: "aa", state: { kind: "applied", creditsAdded: "4950000000", balance: "189450000000" } } as const;
    const balance = (costs: CostTable | undefined, discreet: boolean) => (c: string) => creditBalance(c, costs, discreet, amount, "***");
    expect(outcomeText(applied, balance(undefined, false)).text).toBe("+4,950,000,000 credits. Your balance is 189,450,000,000 credits.");
    expect(outcomeText(applied, balance(undefined, true)).text).toBe("+4,950,000,000 credits. Your balance is ***.");
    expect(outcomeText(applied, balance(COSTS, false)).text).toBe("+4,950,000,000 credits. Your balance is 189450000 duffs.");
    expect(outcomeText(applied, balance(COSTS, true)).text).toBe("+4,950,000,000 credits. Your balance is ***.");
  });

  it("C8 (D7): discreet mode masks the figure and the estimate", () => {
    const m = creditsModel(alice(), credits({ discreet: true }));
    expect(m.figure).toBe("***");
    expect(m.estimate).toBe("≈ *** contact requests · *** profile updates");
  });

  it("C6 (D15): pending rows by their state; Top up waits for them", () => {
    const pending = [
      lock(),
      lock({ txid: "bb", proved: true }),
      lock({ txid: "cc", proved: true, consumptionUnknown: true }),
      lock({ txid: "dd", identity: "Other", identityIndex: 1 }),
      lock({ txid: "ee", proved: true }),
    ];
    // "Credits arrived" only where Platform said the lock was used: the row's flag, or a call's
    // answer this session before the library marked the lock final.
    const m = creditsModel(alice(), credits({ pending, answeredUnknown: new Set(["ee", "aa"]) }));
    expect(m.pending.map((p) => [p.txid, p.line, p.arrived])).toEqual([
      ["aa", PROFILE_COPY.pending.unproved, false],
      ["bb", PROFILE_COPY.pending.proved, false],
      ["cc", PROFILE_COPY.pending.unknown, true],
      ["ee", PROFILE_COPY.pending.unknown, true],
    ]);
    expect(m.pending[0].title).toBe("Top-up of 0.05 tDASH pending");
    expect(m.topUp).toEqual({ disabled: true, reason: PROFILE_COPY.credits.pendingFirst, primary: false });
  });

  it("C7 (D16): a stuck funding of the identity routes to Repair and holds Top up", () => {
    const m = creditsModel(alice(), credits({ stuck: [{ step: "topup/0-1/funding", identityIndex: 0, identity: null, state: "willBeSent" }] }));
    expect(m.stuck).toEqual([{ step: "topup/0-1/funding", title: PROFILE_COPY.stuck.willBeSent, text: PROFILE_COPY.stuck.text }]);
    expect(m.topUp).toMatchObject({ disabled: true, reason: PROFILE_COPY.stuck.willBeSent });
    expect(creditsModel(alice(), credits({ stuck: [{ step: "topup/1-1/funding", identityIndex: 1, identity: "Other", state: "maybeSent" }] })).stuck).toEqual([]);
  });

  it("D8: a pending or stuck top-up of the identity, not another's, holds Top up (Home's Buy credits too)", () => {
    const stuck = { step: "topup/0-1/funding", identityIndex: 0, identity: null, state: "maybeSent" as const };
    expect(holdsTopUp(alice(), [], [])).toBe(false);
    expect(holdsTopUp(alice(), [lock()], [])).toBe(true);
    expect(holdsTopUp(alice(), [], [stuck])).toBe(true);
    expect(holdsTopUp(alice(), [lock({ identity: "Other", identityIndex: 1 })], [{ ...stuck, identityIndex: 1 }])).toBe(false);
  });

  it("D14: a running top-up holds Top up; an unseen result is offered", () => {
    expect(creditsModel(alice(), credits({ toppingUp: true })).topUp).toMatchObject({ disabled: true, reason: PROFILE_COPY.credits.toppingUp });
    expect(creditsModel(alice(), credits({ unseen: true })).unseen).toBe(true);
  });
});

describe("the Home card's credits states (S19, S20, D8)", () => {
  it("a named identity with low or empty credits has a Buy credits card; Join keeps saying joined", () => {
    const low = homeCard(statusInputs({ main: alice({ creditLevel: "low" }) }));
    expect(low).toMatchObject({ state: "lowCredits", title: "Your credit balance is low", action: { kind: "buyCredits", label: "Buy credits" } });
    expect(homeCard(statusInputs({ main: alice({ creditLevel: "empty" }) }))).toMatchObject({ state: "emptyCredits", title: "Your credit balance is fully depleted" });
    expect(homeCard(statusInputs({}))).toBeNull();
    // Names not known yet: no card (DP1-08 D23).
    expect(homeCard(statusInputs({ main: alice({ creditLevel: "low", mainName: null, namesUpdating: true }) }))).toBeNull();
    expect(joinModel(statusInputs({ main: alice({ creditLevel: "low" }) }), null, null)).toEqual({ kind: "joined", username: "alice" });
  });
});

const contest = (patch: Partial<ContestStatus> = {}): ContestStatus => ({
  label: "al",
  state: { kind: "open" },
  endsAt: 1_900_000_000,
  contenders: [
    { identity: "AliceId", votes: 12, isSelf: true },
    { identity: "M", votes: 4, isSelf: false },
    { identity: "N", votes: 3, isSelf: false },
  ],
  lockVotes: 2,
  abstainVotes: null,
  temporaryName: null,
  ...patch,
});

/** The voting card's inputs for `wallet`, through the Join page's gate. */
const KEYED_MAINNET = { watchOnly: false, network: "mainnet" };
const WATCH_ONLY = { watchOnly: true, network: "mainnet" };
const gated = (wallet: { watchOnly: boolean }): InstantInputs => ({ offered: registrationOffered(wallet), nameCredits: "2000000000", asked: null });
const ON = gated(KEYED_MAINNET);
const OFF = gated(WATCH_ONLY);

describe("the request card (IOS-068, AND-071)", () => {
  const date = (secs: number) => `day ${secs}`;
  it("D11: the tally; unknown counts read —", () => {
    expect(tallyRows(contest())).toEqual([
      { label: "Votes for you", value: "12" },
      { label: "Other requests", value: "2, 7 votes" },
      { label: "Votes to block", value: "2" },
      { label: "Abstained", value: "—" },
    ]);
    expect(tallyRows(contest({ contenders: [{ identity: "M", votes: null, isSelf: false }] }))[1].value).toBe("1, — votes");
  });

  it("R1, R2: open, the end or a notice, and the temporary name or an instant one (D13)", () => {
    const open = requestModel("al", 5, contest({ temporaryName: "al2025" }), false, alice(), date, ON);
    expect(open).toMatchObject({ state: "open", title: "Voting – @al", temporary: "al2025", instant: false, readFailed: false });
    expect(open.lines).toEqual([PROFILE_COPY.request.deciding, "Results on day 1900000000.", "Until then you are @al2025."]);
    const noEnd = requestModel("al", null, undefined, false, alice({ mainName: null, names: [] }), date, ON);
    expect(noEnd).toMatchObject({ tally: null, temporary: null, instant: true });
    expect(noEnd.lines).toEqual([PROFILE_COPY.request.deciding, PROFILE_COPY.request.notify]);
    // The main name stands in for a temporary one the contest did not read.
    expect(requestModel("al", null, undefined, false, alice(), date, ON)).toMatchObject({ temporary: "alice", instant: false });
  });

  it("R3–R5: the result read before the status moved", () => {
    expect(requestModel("al", 5, contest({ state: { kind: "won" } }), false, alice(), date)).toMatchObject({ state: "won", title: "@al is yours" });
    expect(requestModel("al", 5, contest({ state: { kind: "lost", winner: "M" } }), false, alice(), date)).toMatchObject({ state: "lost", title: "@al went to another user" });
    expect(requestModel("al", 5, contest({ state: { kind: "lost", winner: null } }), false, alice(), date).title).toBe("Voting on @al ended with no winner");
    expect(requestModel("al", 5, contest({ state: { kind: "locked" } }), false, alice(), date)).toMatchObject({ state: "locked", title: "@al was blocked" });
  });

  it("D4 reversed: the instant offer's gate, Top up when credits fall short, and a temporary name that failed", () => {
    const none = alice({ mainName: null, names: [] });
    expect(requestModel("al", 5, undefined, false, none, date, OFF)).toMatchObject({ instant: false, short: false, failed: null });
    expect(requestModel("al", 5, undefined, false, none, date)).toMatchObject({ instant: false });
    const short = requestModel("al", 5, undefined, false, alice({ mainName: null, names: [], balance: "5" }), date, ON);
    expect(short).toMatchObject({ instant: false, short: true });
    expect(short.lines.at(-1)).toBe("Not enough credits for an instant username.");
    const failed = requestModel("al", 5, undefined, false, none, date, { ...ON, asked: { label: "al2", outcome: "failed" } });
    expect(failed).toMatchObject({ instant: false, failed: "al2" });
    expect(failed.lines.at(-1)).toBe(INSTANT_COPY.failure("al2"));
    const owned = requestModel("al", 5, contest({ temporaryName: "al2" }), false, alice({ mainName: "al2", names: ["al2"] }), date, { ...ON, asked: { label: "al2", outcome: "owned" } });
    expect(owned.lines.at(-1)).toBe(INSTANT_COPY.success("al2", "al"));
  });

  it("gate (a): a watch-only wallet hides the offer, the card action and the won line; a keyed mainnet wallet shows them", () => {
    const none = alice({ mainName: null, names: [] });
    const card = (instant: InstantInputs) =>
      homeCard(statusInputs({ status: { kind: "contestPending", identity: "AliceId", label: "al", endsAt: 5 }, main: none, instant }));
    const won = (instant: InstantInputs) => requestModel("al", 5, contest({ state: { kind: "won" }, temporaryName: "al2" }), false, alice(), date, instant).lines;

    expect(requestModel("al", 5, undefined, false, none, date, ON).instant).toBe(true);
    expect(card(ON)).toMatchObject({ action: { kind: "instantName" }, secondary: { kind: "votingInfo" } });
    expect(won(ON)).toEqual([VOTE_COPY.wonMain("al", "al2")]);

    expect(requestModel("al", 5, undefined, false, none, date, OFF).instant).toBe(false);
    expect(card(OFF)).toMatchObject({ action: { kind: "votingInfo" }, secondary: null });
    expect(won(OFF)).toEqual([PROFILE_COPY.request.won.text]);
  });

  it("vote won: the main name switches, said only on the engine's proved result, never a tally", () => {
    const won = requestModel("al", 5, contest({ state: { kind: "won" }, temporaryName: "al2" }), false, alice(), date, ON);
    expect(won.lines).toEqual(["“al” is now your main username. Your instant username “al2” stays yours too."]);
    // An open contest whose tally leads says nothing of winning.
    const leading = requestModel("al", 5, contest({ temporaryName: "al2", contenders: [{ identity: "AliceId", votes: 99, isSelf: true }] }), false, alice(), date, ON);
    expect(leading.state).toBe("open");
    expect(leading.lines.join(" ")).not.toContain("now your main username");
    // Without a temporary name, or where registration is not offered, the plain line.
    expect(requestModel("al", 5, contest({ state: { kind: "won" } }), false, alice(), date, ON).lines).toEqual([PROFILE_COPY.request.won.text]);
    expect(requestModel("al", 5, contest({ state: { kind: "won" }, temporaryName: "al2" }), false, alice(), date, OFF).lines).toEqual([
      PROFILE_COPY.request.won.text,
    ]);
    // Lost: the instant name stays main; no prompt and no won line.
    expect(requestModel("al", 5, contest({ state: { kind: "lost", winner: "M" }, temporaryName: "al2" }), false, alice(), date, ON).lines).toEqual([]);
  });

  it("R6: a failed read keeps the status's card and says so; another label's read is not used", () => {
    expect(requestModel("al", 5, undefined, true, alice(), date)).toMatchObject({ state: "open", readFailed: true, tally: null });
    expect(requestModel("bob", 5, contest(), false, alice(), date)).toMatchObject({ title: "Voting – @bob", tally: null });
  });
});

describe("outcome rows (IOS-068, D12)", () => {
  const ev = (id: number, kind: DashPayEventInfo["kind"], at: number, reference = "al"): DashPayEventInfo => ({ id, kind, contact: null, reference, at, readAt: null });
  it("the newest three contest outcomes, from both halves of the page", () => {
    const page = {
      pending: [],
      new: [ev(9, "contestWon", 90, "zed"), ev(8, "paymentReceived", 80)],
      earlier: [ev(7, "contestLocked", 70, "bad"), ev(6, "contestLost", 60), ev(5, "contestWon", 50, "old")],
      nextCursor: null,
    };
    expect(outcomeRows(page)).toEqual([
      { id: 9, text: "@zed — yours", at: 90 },
      { id: 7, text: "@bad — blocked", at: 70 },
      { id: 6, text: "@al — went to another user", at: 60 },
    ]);
    expect(outcomeRows(undefined)).toEqual([]);
  });
});
