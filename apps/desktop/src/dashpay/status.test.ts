// The DashPay status surfaces' view model (DP1-08), by parity ID: IOS-065 (the Join DashPay
// banner states, the intro, the voting info; Android's AND-024 Home banner and AND-069–071 voting).

import { describe, expect, it } from "vitest";
import type { DashPayStatus } from "../api/generated/DashPayStatus";
import type { IdentitySummary } from "../api/generated/IdentitySummary";
import type { RegistrationFailure } from "../api/generated/RegistrationFailure";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import type { StartupStatus } from "../api/generated/StartupStatus";
import { COPY, FAQ, failureText, homeCard, JOIN_COPY, joinModel, QUOTE_LABEL, roundedCost, stepOf, VOTING_INFO, type StatusInputs } from "./status";

const amount = (duffs: number) => `${duffs / 1e8} tDASH`;

const inputs = (status: DashPayStatus | undefined, rest: Partial<StatusInputs> = {}): StatusInputs => ({
  status,
  statusError: false,
  main: undefined,
  registration: undefined,
  lock: "unlocked",
  syncPercent: null,
  date: (secs) => `day ${secs}`,
  amount,
  ...rest,
});

const identity = (mainName: string | null, namesUpdating = false) => ({ identity: "A", mainName, namesUpdating }) as IdentitySummary;

const row = (r: Partial<RegistrationStatus> = {}): RegistrationStatus => ({
  draft: "reg-1",
  phase: "fundingSent",
  label: "alice",
  temporaryLabel: null,
  identity: null,
  txid: null,
  waiting: "instantSend",
  holdsKey: false,
  fundsCommitted: true,
  contestEndsAt: null,
  failure: null,
  nameFailure: null,
  profileFailure: null,
  createdAt: 0,
  updatedAt: 0,
  ...r,
});

const failure = (code: string, f: Partial<RegistrationFailure> = {}): RegistrationFailure => ({
  phase: "fundingSent",
  code,
  retryable: true,
  needed: null,
  available: null,
  ...f,
});

const REGISTERING: DashPayStatus = { kind: "registering", draft: "reg-1" };
const startup = (s: StartupStatus): DashPayStatus => ({ kind: "startupIncomplete", startup: s });

describe("IOS-065 / AND-024 Home card: one per banner state", () => {
  it("D2: no card while the status is unread, or for a named or still-updating identity", () => {
    expect(homeCard(inputs(undefined))).toBeNull();
    expect(homeCard(inputs({ kind: "ready", main: "A" }, { main: identity("alice") }))).toBeNull();
    expect(homeCard(inputs({ kind: "ready", main: "A" }, { main: identity(null, true) }))).toBeNull();
    // The identity unread: its names are not known yet.
    expect(homeCard(inputs({ kind: "ready", main: "A" }))).toBeNull();
  });

  it("S2: a failed status read offers Try again and outranks a cached status", () => {
    const c = homeCard(inputs(undefined, { statusError: true }))!;
    expect(c).toMatchObject({ state: "error", tone: "problem", title: COPY.error.title, action: { kind: "retry", disabled: false } });
    expect(homeCard(inputs({ kind: "noIdentity", reason: null }, { statusError: true }))!.state).toBe("error");
  });

  it("S3–S6: every StartupStatus has its card", () => {
    const states: Record<StartupStatus, string> = {
      notRun: "notStarted",
      starting: "looking",
      ready: "looking",
      noIdentity: "looking",
      partialNoIdentity: "lookFailed",
      discoveryFailed: "lookFailed",
      partialAccountsPending: "lookFailed",
      seedBindingUnverified: "lookFailed",
      identityScanIncomplete: "lookFailed",
      identityUnsettled: "unlockToLook",
    };
    for (const [s, state] of Object.entries(states)) expect(homeCard(inputs(startup(s as StartupStatus)))!.state, s).toBe(state);
    expect(homeCard(inputs(startup("starting")))!.busy).toBe(true);
    expect(homeCard(inputs(startup("discoveryFailed")))!.tone).toBe("problem");
  });

  it("S5: Unlock only while mixing-only (fully locked, the lock screen covers the shell)", () => {
    expect(homeCard(inputs(startup("identityUnsettled"), { lock: "unlockedMixingOnly" }))!.action).toMatchObject({ kind: "unlock" });
    expect(homeCard(inputs(startup("identityUnsettled"), { lock: "unlocked" }))!.action).toBeNull();
  });

  it("S7–S9: Join, Join held until synced (with the percentage), watch-only", () => {
    expect(homeCard(inputs({ kind: "noIdentity", reason: null }))).toMatchObject({
      state: "join",
      title: COPY.join.title,
      lines: [COPY.join.message],
      action: { kind: "join", disabled: false },
    });
    const waiting = homeCard(inputs({ kind: "noIdentity", reason: "waitingForSync" }, { syncPercent: 42.25 }))!;
    expect(waiting).toMatchObject({ state: "joinWaiting", note: "Available after sync finishes · 42.3%", action: { kind: "join", disabled: true } });
    expect(homeCard(inputs({ kind: "noIdentity", reason: "waitingForSync" }))!.note).toBe(COPY.joinWaiting.note);
    expect(homeCard(inputs({ kind: "noIdentity", reason: "watchOnly" }))).toMatchObject({ state: "watchOnly", action: null });
  });

  it("S10: a registration whose row is unread or another draft's says only that a username is being created", () => {
    expect(homeCard(inputs(REGISTERING))).toMatchObject({ state: "registering", tone: "pending", busy: true, step: null });
    expect(homeCard(inputs(REGISTERING, { registration: row({ draft: "reg-2" }) }))!.state).toBe("registering");
  });

  it("S11: a running registration shows its username, its step and what it waits for", () => {
    const c = homeCard(inputs(REGISTERING, { registration: row() }))!;
    expect(c).toMatchObject({
      state: "creating",
      tone: "pending",
      title: "Creating – @alice",
      step: { n: 1, text: "Processing payment" },
      lines: ["Waiting for the network to confirm the payment."],
      busy: true,
      action: null,
    });
    expect(homeCard(inputs(REGISTERING, { registration: row({ phase: "nameRequested", waiting: null }) }))).toMatchObject({
      step: { n: 3 },
      lines: [],
    });
  });

  it("S12: waiting for an unlock; Unlock only while mixing-only", () => {
    const r = row({ phase: "keysPrepared", waiting: "unlock" });
    expect(homeCard(inputs(REGISTERING, { registration: r, lock: "unlockedMixingOnly" }))).toMatchObject({
      state: "creatingUnlock",
      lines: ["Unlock the wallet to continue."],
      action: { kind: "unlock" },
    });
    expect(homeCard(inputs(REGISTERING, { registration: r }))!.action).toBeNull();
  });

  it("S13: a failure or an authorization wait pauses it, with Continue into the Join page", () => {
    const failed = row({ phase: "failed", waiting: null, failure: failure("registration.funding_insufficient", { needed: "3002260", available: "1000000" }) });
    expect(homeCard(inputs(REGISTERING, { registration: failed }))).toMatchObject({
      state: "creatingPaused",
      tone: "problem",
      step: { n: 1 },
      lines: [COPY.paused, "Not enough Dash to pay for the username: 0.0300226 tDASH needed, 0.01 tDASH available."],
      action: { kind: "continue" },
    });
    expect(homeCard(inputs(REGISTERING, { registration: row({ waiting: "authorize" }) }))).toMatchObject({
      state: "creatingPaused",
      lines: [COPY.paused, "Waiting for your approval to continue."],
    });
  });

  it("S14: funds of unknown state, or a flow that needs finishing, send the user to Tools ▸ Repair", () => {
    for (const waiting of ["fundingUnknown", "needsFinish"] as const)
      expect(homeCard(inputs(REGISTERING, { registration: row({ waiting }) })), waiting).toMatchObject({
        state: "creatingRepair",
        tone: "problem",
        action: { kind: "repair" },
      });
  });

  it("S15 / AND-069–070: a contested username's vote, its end or a notice, and the temporary name", () => {
    const contest = (endsAt: number | null): DashPayStatus => ({ kind: "contestPending", identity: "A", label: "alice", endsAt });
    expect(homeCard(inputs(contest(1_900_000_000), { main: identity("alice007") }))).toMatchObject({
      state: "voting",
      tone: "pending",
      title: "Voting – @alice",
      lines: [COPY.voting.deciding, "Results on day 1900000000.", "Until then you are @alice007."],
      action: { kind: "votingInfo" },
    });
    // No end known; the temporary name only from a live read.
    expect(homeCard(inputs(contest(null), { main: identity(null, true) }))!.lines).toEqual([COPY.voting.deciding, COPY.voting.notify]);
    expect(homeCard(inputs(contest(null)))!.lines).toEqual([COPY.voting.deciding, COPY.voting.notify]);
  });

  it("S17 (BL-153 D12): an identity without a username is asked to choose one", () => {
    expect(homeCard(inputs({ kind: "ready", main: "A" }, { main: identity(null) }))).toMatchObject({
      state: "noUsername",
      title: COPY.noUsername.title,
      action: { kind: "chooseName", label: COPY.noUsername.action },
    });
  });
});

describe("D4 steps and D5 failure copy", () => {
  it("counts Android's three steps; a failure counts as the phase it stopped in", () => {
    expect(stepOf("draft", null).n).toBe(1);
    expect(stepOf("fundingSent", null).n).toBe(1);
    expect(stepOf("proofWaiting", null)).toEqual({ n: 2, text: "Creating identity" });
    expect(stepOf("identityRegistered", null).n).toBe(3);
    expect(stepOf("failed", failure("x", { phase: "proofWaiting" })).n).toBe(2);
    expect(stepOf("failed", null).n).toBe(3);
  });

  it("says why it stopped, with the amounts where the code carries them", () => {
    expect(failureText(failure("registration.funding_insufficient"), amount)).toBe("Not enough Dash to pay for the username.");
    expect(failureText(failure("registration.islock_timeout"), amount)).toBe("The network did not confirm the payment in time.");
    expect(failureText(failure("platform.timeout"), amount)).toBe("The Dash network did not answer.");
    // Credits are not Dash: no amounts for them.
    expect(failureText(failure("platform.insufficient_credits", { needed: "5", available: "1" }), amount)).toBe("The identity's credits do not cover the username.");
    expect(failureText(failure("name.taken"), amount)).toBe("It stopped with name.taken.");
  });
});

describe("IOS-065 Join page readiness (J1–J7)", () => {
  const noIdentity = (reason: "waitingForSync" | null = null): DashPayStatus => ({ kind: "noIdentity", reason });

  it("D6: the quote's label is valid and not contested (20 characters, a digit 2–9)", () => {
    expect(QUOTE_LABEL).toHaveLength(20);
    expect(QUOTE_LABEL).toMatch(/[2-9]/);
    expect(roundedCost(3_002_260)).toBe(3_100_000);
    expect(roundedCost(3_100_000)).toBe(3_100_000);
  });

  it("J1: the cost, rounded up, and a shortfall against the spendable balance", () => {
    expect(joinModel(inputs(noIdentity()), 3_002_260, 10e8)).toEqual({
      kind: "join",
      cost: JOIN_COPY.cost("0.031 tDASH"),
      shortfall: null,
      waiting: false,
    });
    expect(joinModel(inputs(noIdentity()), 3_002_260, 1_000_000)).toMatchObject({ shortfall: JOIN_COPY.shortfall("0.031 tDASH") });
    // Exactly enough is enough.
    expect(joinModel(inputs(noIdentity()), 3_002_260, 3_002_260)).toMatchObject({ shortfall: null });
  });

  it("J2: Create username waits for sync; J7: an unread or failed quote says only that votes cost more", () => {
    expect(joinModel(inputs(noIdentity("waitingForSync")), 3_002_260, 10e8)).toMatchObject({ waiting: true });
    expect(joinModel(inputs(noIdentity()), null, 10e8)).toEqual({ kind: "join", cost: JOIN_COPY.costUnknown, shortfall: null, waiting: false });
    // The status unread: the intro, its button held.
    expect(joinModel(inputs(undefined), null, null)).toEqual({ kind: "join", cost: JOIN_COPY.costUnknown, shortfall: null, waiting: true });
  });

  it("J3 watch-only; J4–J6 the Home card for the state (J6 a failed read); joined", () => {
    expect(joinModel(inputs({ kind: "noIdentity", reason: "watchOnly" }), null, null)).toEqual({ kind: "watchOnly" });
    expect(joinModel(inputs(REGISTERING, { registration: row() }), null, null)).toMatchObject({ kind: "card", card: { state: "creating" } });
    expect(joinModel(inputs(undefined, { statusError: true }), null, null)).toMatchObject({ kind: "card", card: { state: "error" } });
    expect(joinModel(inputs({ kind: "ready", main: "A" }, { main: identity("alice") }), null, null)).toEqual({ kind: "joined", username: "alice" });
    expect(joinModel(inputs({ kind: "ready", main: "A" }, { main: identity(null, true) }), null, null)).toEqual({ kind: "joined", username: null });
    expect(JOIN_COPY.joined(null)).toBe("You have joined DashPay.");
  });
});

describe("AND-071 / IOS-134 voting info and IOS-129 FAQ", () => {
  it("states the voting rule the network applies and avoids the mobile 'passphrase' (D10)", () => {
    expect(VOTING_INFO.items[0].text).toContain("2–9 or 20 or more characters");
    const text = JSON.stringify([VOTING_INFO, FAQ]);
    expect(text).not.toMatch(/passphrase/i);
    expect(new Set(FAQ.items.map((i) => i.question)).size).toBe(FAQ.items.length);
  });
});
