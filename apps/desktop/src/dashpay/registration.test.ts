// The registration wizard's view model (DP1-09), by parity ID: IOS-066 (choose a username, its
// checklist and availability), IOS-067 (the cost and Confirm), IOS-068 (the progress and result
// screens), and the design's R1–R11.

import { describe, expect, it } from "vitest";
import type { NameAvailability } from "../api/generated/NameAvailability";
import type { RegistrationFailure } from "../api/generated/RegistrationFailure";
import type { RegistrationQuote } from "../api/generated/RegistrationQuote";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import type { UsernameCheck } from "../api/generated/UsernameCheck";
import { appError } from "../onboarding/testWorld";
import {
  availabilityLine,
  canChoose,
  confirmModel,
  isRunning,
  namedModel,
  pendingModel,
  REG_COPY,
  screenModel,
  platformRefusalText,
  startErrorText,
  type ScreenInputs,
} from "./registration";
import { PAY_WITH_COPY } from "./payWith";
import { COPY, WAITS } from "./status";

const amount = (duffs: number) => `${duffs / 1e8} tDASH`;

const check = (valid: boolean, contested = false): UsernameCheck => ({ valid, normalized: "x", contested, rules: [] });

const quote = (q: Partial<RegistrationQuote> = {}): RegistrationQuote => ({
  contested: false,
  lockDuffs: 3_000_000,
  feeDuffs: 2_260,
  totalDuffs: 3_002_260,
  remainingCredits: "1000000000",
  availableDuffs: 10e8,
  grant: { maxDuffs: 3_002_261, maxCredits: "2000000000" },
  readiness: null,
  platform: null,
  ...q,
});

const row = (r: Partial<RegistrationStatus> = {}): RegistrationStatus => ({
  draft: "reg-1",
  phase: "fundingSent",
  label: "alice",
  temporaryLabel: null,
  identity: null,
  txid: "f0".repeat(32),
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

const I: ScreenInputs = { contestPending: false, contestEndsAt: null, date: (s) => `day ${s}`, amount, discreet: false };
const kinds = (r: RegistrationStatus, i: ScreenInputs = I) => {
  const m = screenModel(r, i);
  return [m.kind, ...m.actions.map((a) => a.kind)];
};

describe("IOS-066 step 1: the checklist and availability (F4, D3, D16)", () => {
  it("says nothing of availability until the label is valid, then Checking… while the typing settles", () => {
    expect(availabilityLine(check(false), undefined, false)).toBeNull();
    expect(availabilityLine(undefined, undefined, true)).toBeNull();
    expect(availabilityLine(check(true), undefined, true)).toEqual({ text: "Checking…", tone: "pending" });
    expect(availabilityLine(check(true), { kind: "available", contested: false }, true)).toMatchObject({ tone: "pending" });
  });

  it("each answer has its line; taken, locked and invalid are problems", () => {
    expect(availabilityLine(check(true), { kind: "available", contested: false }, false)).toEqual({ text: "Username available", tone: "ok" });
    expect(availabilityLine(check(true), { kind: "taken", owner: null }, false)).toMatchObject({ text: "Username taken", tone: "problem" });
    expect(availabilityLine(check(true), { kind: "locked" }, false)).toMatchObject({ tone: "problem" });
    expect(availabilityLine(check(true), { kind: "invalid", rules: ["minLength"] }, false)).toMatchObject({ tone: "problem" });
    expect(availabilityLine(check(true), { kind: "contestOpen", endsAt: null, contenders: 1 }, false)?.text).toMatch(/let the network vote/);
    expect(availabilityLine(check(true), { kind: "unknown" }, false)).toMatchObject({ text: "Could not check availability. You can still continue.", tone: "neutral" });
  });

  it("D3: Continue on available, an open contest or an unknown answer; never while pending or refused", () => {
    expect(canChoose(check(true), { kind: "available", contested: true }, false)).toBe(true);
    expect(canChoose(check(true), { kind: "contestOpen", endsAt: 1, contenders: 2 }, false)).toBe(true);
    expect(canChoose(check(true), { kind: "unknown" }, false)).toBe(true);
    expect(canChoose(check(true), { kind: "available", contested: false }, true)).toBe(false);
    expect(canChoose(check(true), undefined, false)).toBe(false);
    expect(canChoose(check(false), { kind: "available", contested: false }, false)).toBe(false);
    const refusals: NameAvailability[] = [{ kind: "taken", owner: null }, { kind: "locked" }, { kind: "invalid", rules: [] }];
    for (const refused of refusals)
      expect(canChoose(check(true), refused, false), refused.kind).toBe(false);
  });
});

describe("IOS-067 step 2: the cost (D2, D5)", () => {
  it("the quote's total in Dash, the credits line, and the voting line for a contested name", () => {
    expect(confirmModel(quote(), amount)).toEqual({ cost: "0.0300226 tDASH", lines: [REG_COPY.confirm.credits], shortfall: null });
    expect(confirmModel(quote({ contested: true }), amount).lines).toEqual([REG_COPY.confirm.credits, REG_COPY.confirm.contested]);
  });

  it("D5: a shortfall when what the funding can spend now is below the total, none at exactly the total", () => {
    expect(confirmModel(quote({ availableDuffs: 3_002_259 }), amount).shortfall).toBe("You need at least 0.0300226 tDASH to create this username.");
    expect(confirmModel(quote({ availableDuffs: 3_002_260 }), amount).shortfall).toBeNull();
  });

  it("C-funding F9: from the Platform balance, what leaves it, the identity's funding and the fee, never the reserve", () => {
    const platform = { fundingCredits: "2000000000", feeCredits: "41500001", feeReserveCredits: "124500000", debitCredits: "2124500000" };
    const model = confirmModel(quote({ totalDuffs: 0, availableDuffs: 0, platform }), amount);
    expect(model).toEqual({
      cost: "0.02041501 tDASH",
      lines: ["About 0.02041501 tDASH leaves your Platform balance: 0.02 tDASH for your identity and a Platform fee of about 0.00041501 tDASH. Your identity starts with a balance of 0.02 tDASH."],
      shortfall: null,
    });
    expect(confirmModel(quote({ contested: true, totalDuffs: 0, availableDuffs: 0, platform }), amount).lines[1]).toBe(REG_COPY.confirm.contested);
  });
});

describe("IOS-068 the progress and result screens (R1–R11)", () => {
  it("R1: a running row: its step and wait, busy, Let me know when it's done", () => {
    const m = screenModel(row(), I);
    expect(m).toMatchObject({ kind: "progress", busy: true, step: { n: 1 } });
    expect(m.lines).toEqual([REG_COPY.progress.message("alice"), "Waiting for the network to confirm the payment.", REG_COPY.progress.close]);
    expect(m.actions).toEqual([{ kind: "home", label: "Let me know when it's done" }]);
    expect(isRunning(row())).toBe(true);
  });

  it("R2, R3: an idle row waits for Continue's grant or the full unlock (D12)", () => {
    expect(kinds(row({ waiting: "authorize", holdsKey: false }))).toEqual(["paused", "continue"]);
    expect(kinds(row({ waiting: "unlock", holdsKey: false }))).toEqual(["unlock", "unlock"]);
    expect(isRunning(row({ waiting: "authorize" }))).toBe(false);
    expect(isRunning(row({ waiting: "unlock" }))).toBe(false);
  });

  it("#685: a held Platform payment's screen offers Continue, then Move to Dash wallet", () => {
    const m = screenModel(row({ waiting: "paymentHeld", running: false, fundsCommitted: true, fromPlatform: true }), I);
    expect(m).toMatchObject({ kind: "held", busy: false, step: { n: 1 } });
    expect(m.lines).toEqual([REG_COPY.progress.message("alice"), WAITS.paymentHeld]);
    expect(m.actions).toEqual([
      { kind: "continue", label: "Continue" },
      { kind: "moveToCore", label: "Move to Dash wallet" },
    ]);
  });

  it("D21, D26: an idle row waiting for a proof (after a relaunch) is paused, Continue", () => {
    expect(kinds(row({ waiting: "chainLock", running: false }))).toEqual(["paused", "continue"]);
    expect(isRunning(row({ waiting: "chainLock", running: false }))).toBe(false);
    // A run is progress and polled whether or not it holds a key of its own (review r2 H1).
    for (const holdsKey of [true, false]) {
      expect(kinds(row({ waiting: "chainLock", holdsKey }))[0]).toBe("progress");
      expect(isRunning(row({ waiting: "instantSend", holdsKey }))).toBe(true);
    }
    expect(kinds(row({ phase: "nameRequested", waiting: null, holdsKey: false }))[0]).toBe("progress");
  });

  it("D23: a draft started here and not read yet is busy, never the Join page", () => {
    expect(pendingModel()).toMatchObject({ kind: "progress", busy: true, step: null, actions: [{ kind: "home" }] });
    // A failed read is not busy forever (review r2 L2).
    expect(pendingModel(true)).toMatchObject({ busy: false, lines: [REG_COPY.progress.readFailed], actions: [{ kind: "home" }] });
  });

  it("R4, R5: funds waiting for a decision say they are safe and point to Repair", () => {
    const unknown = screenModel(row({ waiting: "fundingUnknown" }), I);
    expect([unknown.kind, ...unknown.actions.map((a) => a.kind)]).toEqual(["repair", "repair"]);
    expect(unknown.lines).toEqual([COPY.repair.unknown, REG_COPY.recovery]);
    const finish = screenModel(row({ waiting: "needsFinish" }), I);
    expect([finish.kind, ...finish.actions.map((a) => a.kind)]).toEqual(["repair", "finish", "repair"]);
    expect(finish.lines).toContain(REG_COPY.recovery);
    // A failed row whose funding is unknown is that first.
    expect(kinds(row({ phase: "failed", waiting: "fundingUnknown", failure: failure("registration.islock_timeout", { retryable: false }) }))[0]).toBe("repair");
  });

  it("R6: retryable: Continue; Choose another username only while nothing is committed (D7)", () => {
    const funded = screenModel(row({ phase: "failed", waiting: null, failure: failure("platform.unavailable") }), I);
    expect([funded.kind, ...funded.actions.map((a) => a.kind)]).toEqual(["retry", "continue"]);
    expect(funded.lines).toContain(REG_COPY.recovery);
    const unfunded = row({ phase: "failed", waiting: null, fundsCommitted: false, failure: failure("registration.funding_insufficient", { needed: "3002260", available: "1000000" }) });
    expect(kinds(unfunded)).toEqual(["retry", "continue", "another"]);
    expect(screenModel(unfunded, I).lines).toContain("Not enough Dash to pay for the username: 0.0300226 tDASH needed, 0.01 tDASH available.");
    expect(screenModel(unfunded, { ...I, discreet: true }).lines.join(" ")).not.toContain("0.01 tDASH");
  });

  it("R7, R8: failed for good: another username if nothing is committed, else the recovery copy and Repair", () => {
    expect(kinds(row({ phase: "failed", waiting: null, fundsCommitted: false, failure: failure("registration.unsupported", { retryable: false }) }))).toEqual([
      "failed",
      "another",
    ]);
    const stopped = screenModel(row({ phase: "failed", waiting: null, failure: failure("registration.islock_timeout", { retryable: false }) }), I);
    expect([stopped.kind, ...stopped.actions.map((a) => a.kind)]).toEqual(["stopped", "repair", "another"]);
    expect(stopped.lines).toContain(REG_COPY.recovery);
  });

  it("C-funding F10: a paid failure from the Platform balance says what was kept, and offers a new registration", () => {
    const refunded = screenModel(row({ phase: "failed", waiting: null, fundsCommitted: false, fromPlatform: true, failure: failure("registration.refunded", { retryable: false }) }), I);
    expect([refunded.kind, refunded.title, ...refunded.actions.map((a) => `${a.kind}:${a.label}`)]).toEqual(["failed", "Registration failed", "another:Start a new registration"]);
    expect(refunded.lines).toEqual(["Dash Platform could not create the identity with this wallet's keys and kept a Platform fee and a penalty from your Platform balance for the attempt. The rest of your Platform balance was not used."]);
    // Another failure keeps its label.
    const failed = screenModel(row({ phase: "failed", waiting: null, fundsCommitted: false, fromPlatform: true, failure: failure("platform.timeout", { retryable: false }) }), I);
    expect(failed.actions.map((a) => `${a.kind}:${a.label}`)).toEqual(["another:Choose another username"]);
  });

  it("R9, R10, R11: done greets the name; a refused name keeps the identity; a contested one shows the vote", () => {
    const done = screenModel(row({ phase: "done", waiting: null, identity: "A" }), I);
    expect(done).toMatchObject({
      kind: "done",
      title: "Welcome to DashPay, @alice",
      lines: ["Your username is ready. Friends can now find you and pay you by name."],
      step: null,
      busy: false,
      actions: [{ kind: "wallet", label: "Go to wallet" }],
    });
    const taken = screenModel(row({ phase: "done", waiting: null, identity: "A", nameFailure: "name.taken" }), I);
    expect(taken).toMatchObject({ kind: "nameFailed", title: "Your identity is ready", lines: ["@alice could not be registered. Username was taken by another user."] });
    expect(taken.actions.map((a) => a.kind)).toEqual(["another"]);
    expect(kinds(row({ phase: "done", waiting: null }), { ...I, contestPending: true, contestEndsAt: 7 })).toEqual(["voting", "done", "votingInfo"]);
    expect(screenModel(row({ phase: "contested", waiting: null, contestEndsAt: 9 }), I).lines).toEqual([REG_COPY.voting.message("alice"), "Results on day 9."]);
    expect(isRunning(row({ phase: "done", waiting: null }))).toBe(false);
  });

  it("D6: a name from credits ends registered or with its contest", () => {
    expect(namedModel("bob", { kind: "registered" }, I.date)).toMatchObject({ kind: "done", title: "Welcome to DashPay, @bob" });
    expect(namedModel("bob", { kind: "contestStarted", endsAt: 5 }, I.date)).toMatchObject({ kind: "voting", lines: [REG_COPY.voting.message("bob"), "Results on day 5."] });
    expect(namedModel("bob", { kind: "contestStarted", endsAt: null }, I.date).lines[1]).toBe(COPY.voting.notify);
  });
});

describe("refusals of a start", () => {
  it("say the amounts unless discreet, and the m4 copy for a recent unknown funding", () => {
    const short = appError("registration.funding_insufficient", { kind: "duffs", needed: 3_002_260, available: 1_000_000 });
    expect(startErrorText(short, amount)).toBe("Not enough Dash to pay for the username: 0.0300226 tDASH needed, 0.01 tDASH available.");
    expect(startErrorText(short, amount, true)).toBe("Not enough Dash to pay for the username: 0.0300226 tDASH needed.");
    expect(startErrorText(appError("registration.funding_unknown_recent", { kind: "draftWait", draft: "d", waitSecs: 600 }), amount)).toBe(
      "The earlier registration's payment was sent less than an hour ago. Try again in 10 minutes.",
    );
    expect(startErrorText(appError("registration.funding_unknown_recent", { kind: "draftWait", draft: "d", waitSecs: null }), amount)).toMatch(/can't yet tell/);
    expect(startErrorText(appError("name.taken"), amount)).toBe("Username taken. Choose another one.");
    expect(startErrorText(new Error("boom"), amount)).toBe("boom");
  });

  it("from the Platform balance, its own refusals say what the balance lacks or what holds it", () => {
    const short = appError("platform.insufficient_credits", { kind: "credits", needed: "2124500000", available: "1" });
    expect(startErrorText(short, amount, false, true)).toBe("You need at least 0.021245 tDASH to create this username.");
    expect(startErrorText(short, amount)).toBe("The identity's credits do not cover this username.");
    expect(startErrorText(appError("credits.move_pending", { kind: "artifact", artifact: "m" }), amount, false, true)).toBe(PAY_WITH_COPY.movePending);
    expect(startErrorText(appError("platform.feature_off", { kind: "feature", feature: "platform_registration" }), amount, false, true)).toBe(PAY_WITH_COPY.notOffered);
    expect(startErrorText(appError("registration.quote_changed"), amount, false, true)).toBe("The cost changed. Review it and confirm again. Nothing was paid.");
    expect(startErrorText(appError("name.taken"), amount, false, true)).toBe("Username taken. Choose another one.");
    expect(platformRefusalText(appError("platform.unavailable"), amount)).toBeNull();
  });
});

describe("REG_COPY.finished", () => {
  const report = { resumed: 3, completed: 2, stillPending: 1, alreadyUsed: 0, byKind: [], unread: [] };

  it("names the already-used count only when there is one", () => {
    expect(REG_COPY.finished(report)).toBe("3 resumed, 2 completed, 1 still pending.");
    expect(REG_COPY.finished({ ...report, alreadyUsed: 1 })).toBe("3 resumed, 2 completed, 1 still pending, 1 already used.");
    expect(REG_COPY.finished({ ...report, unread: ["shielded"] })).toBe("3 resumed, 2 completed, 1 still pending. Shielded transfers couldn't be checked; try again.");
  });

  it("a Shielded-only report reads as before; one with an Add from Dash wallet row names each kind", () => {
    const shielded = { kind: "shielded" as const, resumed: 3, completed: 2, stillPending: 1 };
    expect(REG_COPY.finished({ ...report, byKind: [shielded] })).toBe("3 resumed, 2 completed, 1 still pending.");
    const fund = { kind: "platformAddress" as const, resumed: 1, completed: 1, stillPending: 0 };
    expect(REG_COPY.finished({ ...report, resumed: 1, completed: 1, stillPending: 0, byKind: [fund] })).toBe("1 resumed, 1 completed, 0 still pending. Add from Dash wallet: 1 completed.");
    expect(REG_COPY.finished({ ...report, byKind: [shielded, { ...fund, completed: 0, stillPending: 1 }] })).toBe(
      "3 resumed, 2 completed, 1 still pending. Shielded: 2 completed, 1 still pending. Add from Dash wallet: 0 completed, 1 still pending.",
    );
  });
});
