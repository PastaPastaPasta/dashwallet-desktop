// The DashPay status surfaces' view model (DP1-08), by parity ID: IOS-065 (the Join DashPay
// banner states, the intro, the voting info; Android's AND-024 Home banner and AND-069–071 voting).

import { describe, expect, it } from "vitest";
import type { DashPayStatus } from "../api/generated/DashPayStatus";
import type { IdentitySummary } from "../api/generated/IdentitySummary";
import type { RegistrationFailure } from "../api/generated/RegistrationFailure";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import type { StartupStatus } from "../api/generated/StartupStatus";
import { CLAIM_REFUSAL, COPY, FAQ, failureText, homeCard, JOIN_COPY, heldWithBoth, joinModel, namesState, QUOTE_LABEL, READINESS_FAILED, roundedCost, shieldedFunds, stepOf, VOTING_INFO, WAITS, type StatusInputs } from "./status";
import type { ShieldedCheckName } from "../api/generated/ShieldedCheckName";
import type { ShieldedReadiness } from "../api/generated/ShieldedReadiness";
import { t } from "../i18n";
import { INSTANT_COPY, VOTE_COPY, type InstantInputs } from "./instantOffer";

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
  discreet: false,
  ...rest,
});

const identity = (mainName: string | null, namesUpdating = false, namesCheck: IdentitySummary["namesCheck"] = null) =>
  ({ identity: "A", mainName, namesUpdating, namesCheck }) as IdentitySummary;

const row = (r: Partial<RegistrationStatus> = {}): RegistrationStatus => ({
  draft: "reg-1",
  phase: "fundingSent",
  label: "alice",
  temporaryLabel: null,
  identity: null,
  txid: null,
  waiting: "instantSend",
  holdsKey: true,
  running: true,
  fundsCommitted: true,
  contestEndsAt: null,
  failure: null,
  nameFailure: null,
  profileFailure: null,
  invitation: null,
  fromPlatform: false,
  fromShielded: false,
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
    // D24: nor does the Join page offer Create username on a cached answer.
    expect(joinModel(inputs({ kind: "noIdentity", reason: null }, { statusError: true }), 100, 100)).toMatchObject({ kind: "card", card: { state: "error" } });
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
    // A seed that does not match the identity's keys is not fixed by a restart.
    expect(homeCard(inputs(startup("seedBindingUnverified")))).toMatchObject({ title: COPY.unverified.title, lines: [COPY.unverified.message] });
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

  it("D21, D26: an idle row waiting for a proof or the network (after a relaunch) is paused, not running", () => {
    for (const waiting of ["chainLock", "network"] as const) {
      expect(homeCard(inputs(REGISTERING, { registration: row({ waiting, holdsKey: false, running: false }) }))).toMatchObject({
        state: "creatingPaused",
        lines: ["Registration paused", "Waiting for your approval to continue."],
        action: { kind: "continue" },
      });
    }
    // A run on an unlocked vault holds no key of its own and still runs (review r2 H1).
    for (const waiting of ["instantSend", "chainLock", "network", null] as const) {
      expect(homeCard(inputs(REGISTERING, { registration: row({ waiting, holdsKey: false, running: true }) }))!.state).toBe("creating");
    }
    // An idle row held for sync or the DashPay start shows progress until the hold lifts.
    expect(homeCard(inputs(REGISTERING, { registration: row({ waiting: "sync", holdsKey: false, running: false }) }))!.state).toBe("creating");
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
    // Discreet mode leaves out what the wallet holds.
    expect(homeCard(inputs(REGISTERING, { registration: failed, discreet: true }))!.lines[1]).toBe(
      "Not enough Dash to pay for the username: 0.0300226 tDASH needed.",
    );
    expect(homeCard(inputs(REGISTERING, { registration: row({ waiting: "authorize" }) }))).toMatchObject({
      state: "creatingPaused",
      lines: [COPY.paused, "Waiting for your approval to continue."],
    });
  });

  it("S14: funds of unknown state, or a flow that needs finishing, send the user to Tools ▸ Repair", () => {
    for (const [waiting, what] of [
      ["fundingUnknown", COPY.repair.unknown],
      ["needsFinish", COPY.repair.finish],
    ] as const)
      expect(homeCard(inputs(REGISTERING, { registration: row({ waiting }) })), waiting).toMatchObject({
        state: "creatingRepair",
        tone: "problem",
        lines: [what, COPY.repair.message],
        action: { kind: "repair" },
      });
  });

  it("#685: a held Platform payment offers Continue and Move to Dash wallet", () => {
    const held = row({ waiting: "paymentHeld", running: false, fundsCommitted: true, fromPlatform: true });
    expect(homeCard(inputs(REGISTERING, { registration: held }))).toMatchObject({
      state: "creatingHeld",
      tone: "problem",
      step: { n: 1 },
      lines: [heldWithBoth()],
      action: { kind: "continue", label: "Continue" },
      secondary: { kind: "moveToCore", label: "Move to Dash wallet" },
    });
    // The line beside both names both; the plain wait line names neither (MG).
    expect(heldWithBoth()).toContain("Continue");
    expect(heldWithBoth()).toContain("Move to Dash wallet");
    expect(WAITS.paymentHeld).not.toContain("Continue");
    expect(WAITS.paymentHeld).not.toContain("Move to Dash wallet");
    // Unknown is still a running wait, with no way out offered.
    expect(homeCard(inputs(REGISTERING, { registration: row({ waiting: "paymentUnknown", running: true, fromPlatform: true }) }))).toMatchObject({ state: "creating", secondary: null });
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

  it("D23: only names the engine has fetched say there is none; pending or failed ones ask for nothing", () => {
    expect(namesState(undefined)).toBe("updating");
    expect(namesState(identity("alice", false, "pending"))).toBe("named");
    expect(namesState(identity(null, true))).toBe("updating");
    expect(namesState(identity(null, false, "pending"))).toBe("updating");
    expect(namesState(identity(null, false, "failed"))).toBe("unknown");
    expect(namesState(identity(null))).toBe("none");
    for (const check of ["pending", "failed"] as const) {
      expect(homeCard(inputs({ kind: "ready", main: "A" }, { main: identity(null, false, check) })), check).toBeNull();
      expect(joinModel(inputs({ kind: "ready", main: "A" }, { main: identity(null, false, check) }), null, null), check).toEqual({
        kind: "joined",
        username: null,
      });
    }
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

  it("DP5-03 D10: a claim checks, then creates the identity; nothing is paid", () => {
    expect(stepOf("draft", null, true)).toEqual({ n: 1, text: "Checking the invitation" });
    expect(stepOf("keysPrepared", null, true)).toEqual({ n: 2, text: "Creating identity" });
    expect(stepOf("identityRegistered", null, true)).toEqual({ n: 3, text: "Registering username" });
    expect(stepOf("failed", failure("x", { phase: "keysPrepared" }), true).n).toBe(2);
    expect(stepOf("draft", null).text).toBe("Processing payment");
  });

  it("says why it stopped, with the amounts where the code carries them", () => {
    expect(failureText(failure("registration.funding_insufficient"), amount)).toBe("Not enough Dash to pay for the username.");
    expect(failureText(failure("registration.islock_timeout"), amount)).toBe("The network did not confirm the payment in time.");
    expect(failureText(failure("platform.timeout"), amount)).toBe("The Dash network did not answer.");
    expect(failureText(failure("rescan_interrupted"), amount)).toBe("The last rescan was interrupted. Finish it in Repair ▸ Rescan before sending.");
    // Credits are not Dash: no amounts for them.
    expect(failureText(failure("platform.insufficient_credits", { needed: "5", available: "1" }), amount)).toBe("The identity's credits do not cover the username.");
    expect(failureText(failure("platform.grant_exceeded"), amount)).toBe("The cost went above what you approved.");
    expect(failureText(failure("name.taken"), amount)).toBe("It stopped with name.taken.");
    // C-funding F10: a paid failure from the Platform balance; another source's has no copy yet.
    expect(failureText(failure("registration.refunded", { retryable: false }), amount, false, true)).toBe(
      "Dash Platform could not create the identity with this wallet's keys and kept a Platform fee and a penalty from your Platform balance for the attempt. The rest of your Platform balance was not used.",
    );
    expect(failureText(failure("registration.refunded", { retryable: false }), amount)).toBe("It stopped with registration.refunded.");
    // A run stopped by a changed Platform plan has its own line, not the code or Confirm's.
    const changed = failureText(failure("registration.quote_changed", { retryable: false }), amount, false, true);
    expect(changed).toBe(
      "No identity was paid for. If you chose Move to Dash wallet for this registration, those credits are in your Dash wallet now, less that move's fee. Review the cost and confirm to try again.",
    );
    expect(changed).not.toBe(t("dashpay.registration.startError.quoteChanged"));
    expect(changed).not.toContain("registration.quote_changed");
    // DP5-03 (Opus r1 L1): a claim's refusals, on the Join page's failed row too.
    expect(failureText(failure("invitation.already_has_identity"), amount)).toBe(CLAIM_REFUSAL.hasIdentity);
    expect(failureText(failure("invitation.claimed"), amount)).toBe(`${CLAIM_REFUSAL.claimed.title}. ${CLAIM_REFUSAL.claimed.text}`);
    expect(failureText(failure("invitation.expired"), amount)).toBe(`${CLAIM_REFUSAL.expired.title}. ${CLAIM_REFUSAL.expired.text}`);
    expect(failureText(failure("invitation.invalid"), amount)).toBe(CLAIM_REFUSAL.invalid);
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

  it("J1: the cost, rounded up, and a shortfall only when what the funding can spend does not cover it (D18, DP1-09 D5)", () => {
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

  it("C-funding F16: no shortfall while the Shielded balance can pay or is unread; neither can, both balances, or neither in discreet mode", () => {
    const ready = (checks: Partial<Record<ShieldedCheckName, boolean>> = {}, denomination: string | null = "3000000000"): ShieldedReadiness => ({
      denomination,
      checks: (["poolActive", "featureOn", "denomination", "balance", "noUnsettledSpend", "synced", "rested", "poolSize"] as const).map((check) => ({ check, ready: checks[check] ?? true })),
      readyAt: null,
      poolNotes: null,
      poolRequired: 250,
      change: null,
    });
    const short = (r: ShieldedReadiness | null, shielded: number | null, discreet = false) => {
      const m = joinModel(inputs(noIdentity(), { discreet }), 3_002_260, 1_000_000, shieldedFunds(r, shielded));
      return m.kind === "join" ? m.shortfall : m.kind;
    };
    // Enough in Shielded, even while it rests: the privacy step says when it can pay.
    expect(short(ready(), 10e8)).toBeNull();
    expect(short(ready({ rested: false }), 10e8)).toBeNull();
    expect(short(null, null)).toBeNull();
    expect(short(ready({ balance: false }), 100_000)).toBe(
      "You need at least 0.031 tDASH to create a username. Your Dash wallet has 0.01 tDASH. Your Shielded balance has 0.001 tDASH.",
    );
    expect(short(ready({ balance: false }), 100_000, true)).toBe(
      "You need at least 0.031 tDASH to create a username. Neither your Dash wallet nor your Shielded balance has enough.",
    );
    // A network without Shielded registrations keeps the plain shortfall.
    expect(short(ready({ poolActive: false }), null)).toBe(JOIN_COPY.shortfall("0.031 tDASH"));
    expect(short(ready({}, null), null)).toBe(JOIN_COPY.shortfall("0.031 tDASH"));
    // A failed readiness quote is not offered, never unread: the plain shortfall.
    expect(short(READINESS_FAILED, null)).toBe(JOIN_COPY.shortfall("0.031 tDASH"));
    // The Shielded balance not read yet: no amount for it.
    expect(short(ready({ balance: false }), null)).toBe(JOIN_COPY.shortfallBothDiscreet("0.031 tDASH"));
  });

  it("J2: Create username waits for sync, with no shortfall from a partial balance; J7: an unread or failed quote says only that votes cost more", () => {
    expect(joinModel(inputs(noIdentity("waitingForSync")), 3_002_260, 10e8)).toMatchObject({ waiting: true });
    expect(joinModel(inputs(noIdentity("waitingForSync")), 3_002_260, 0)).toMatchObject({ waiting: true, shortfall: null });
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

describe("the voting card's instant username (D4 reversed)", () => {
  const CONTEST: DashPayStatus = { kind: "contestPending", identity: "A", label: "alice", endsAt: 1_900_000_000 };
  const named = (mainName: string | null, balance: string | null = "5000000000") =>
    ({ identity: "A", mainName, names: mainName ? [mainName] : [], namesUpdating: false, namesCheck: null, balance }) as IdentitySummary;
  const on: InstantInputs = { offered: true, nameCredits: "2000000000", asked: null };
  const base = [COPY.voting.deciding, "Results on day 1900000000."];

  it("no temporary name: the hint and Create instant username, voting info beside it", () => {
    expect(homeCard(inputs(CONTEST, { main: named(null), instant: on }))).toMatchObject({
      state: "voting",
      lines: [...base, VOTE_COPY.instantHint("alice")],
      action: { kind: "instantName", label: "Create instant username" },
      secondary: { kind: "votingInfo" },
    });
  });

  it("credits short of a name: the short line and Top up", () => {
    expect(homeCard(inputs(CONTEST, { main: named(null, "100"), instant: on }))).toMatchObject({
      lines: [...base, "Not enough credits for an instant username."],
      action: { kind: "buyCredits", label: "Top up" },
      secondary: { kind: "votingInfo" },
    });
  });

  it("hidden where registration is not offered, and while the names are unread or a temporary name is held", () => {
    const quiet = { action: { kind: "votingInfo" }, secondary: null };
    expect(homeCard(inputs(CONTEST, { main: named(null), instant: { ...on, offered: false } }))).toMatchObject(quiet);
    expect(homeCard(inputs(CONTEST, { main: named(null) }))).toMatchObject(quiet);
    expect(homeCard(inputs(CONTEST, { main: identity(null, true), instant: on }))).toMatchObject(quiet);
    expect(homeCard(inputs(CONTEST, { main: named("alice007"), instant: on }))).toMatchObject({ ...quiet, lines: [...base, "Until then you are @alice007."] });
  });

  it("the registration's temporary name: success once read, its failure with Try again, nothing until a read says", () => {
    const asked = (outcome: "owned" | "failed" | null): InstantInputs => ({ ...on, asked: { label: "alice2", outcome } });
    expect(homeCard(inputs(CONTEST, { main: named("alice2"), instant: asked("owned") }))).toMatchObject({
      lines: [...base, INSTANT_COPY.success("alice2", "alice")],
      action: { kind: "votingInfo" },
    });
    expect(homeCard(inputs(CONTEST, { main: named(null), instant: asked("failed") }))).toMatchObject({
      tone: "problem",
      lines: [...base, INSTANT_COPY.failure("alice2")],
      action: { kind: "instantRetry", label: "Try again" },
      secondary: { kind: "votingInfo" },
      retryLabel: "alice2",
    });
    expect(homeCard(inputs(CONTEST, { main: named(null), instant: asked(null) }))).toMatchObject({ lines: base, action: { kind: "votingInfo" }, secondary: null });
    // Failed with credits short of the name: Top up, as for a new one, not Try again.
    expect(homeCard(inputs(CONTEST, { main: named(null, "100"), instant: asked("failed") }))).toMatchObject({
      tone: "problem",
      lines: [...base, INSTANT_COPY.failure("alice2"), "Not enough credits for an instant username."],
      action: { kind: "buyCredits", label: "Top up" },
      secondary: { kind: "votingInfo" },
    });
  });

  it("while the registrations are not read: neither Create nor Try again", () => {
    const quiet = { lines: base, action: { kind: "votingInfo" }, secondary: null };
    expect(homeCard(inputs(CONTEST, { main: named(null), instant: { ...on, asked: undefined } }))).toMatchObject(quiet);
  });

  it("the Join page shows the same card", () => {
    expect(joinModel(inputs(CONTEST, { main: named(null), instant: on }), null, null)).toMatchObject({ kind: "card", card: { action: { kind: "instantName" } } });
  });
});
