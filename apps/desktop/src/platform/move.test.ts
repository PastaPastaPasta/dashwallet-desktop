// "Move to Dash wallet": everything moves (`all`) under the quote's platformOp grant; the sheet
// can close while the call runs and reopens to its result; refusals say what to do.

import { afterEach, describe, expect, it, vi } from "vitest";
import type { MoveToCoreOutcome } from "../api/generated/MoveToCoreOutcome";
import type { PlatformActivity } from "../api/generated/PlatformActivity";
import { appError, WALLET_A } from "../onboarding/testWorld";
import { fakeVault } from "../tools/testApi";
import { hasPendingMove, hasUnansweredOp, sendHolds, invalidArgumentText, moveBlock, moveCanRetry, moveErrorText, moveFor, MOVE_COPY, onMoveSettled, PlatformMoveFlow, resetPlatformMoves } from "./move";

const TO = "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf";
const OUTCOME: MoveToCoreOutcome = { id: "pm-1", credits: "40000000000", expectedDuffs: 39_998_000, to: TO, state: "done", feeCredits: "2000000" };
const GRANT = { maxDuffs: 0, maxCredits: "40000000000" };
const credits = (c: string) => `${Number(BigInt(c) / 1000n) / 1e8} tDASH`;

function setup(lock = { encrypted: false }) {
  const vault = fakeVault(lock);
  const calls: string[] = [];
  let answer: ((o: MoveToCoreOutcome) => void) | null = null;
  const api = {
    ...vault.api,
    platformMoveToCore: vi.fn(
      (_walletId: string, amount: { kind: string }, to: string, grant: string) =>
        new Promise<MoveToCoreOutcome>((resolve) => {
          calls.push(`move ${amount.kind} ${to} ${grant}`);
          answer = resolve;
        }),
    ),
  };
  const flow = new PlatformMoveFlow(api, WALLET_A);
  return { flow, api, vault, calls, answer: (o: MoveToCoreOutcome) => answer!(o) };
}

afterEach(() => resetPlatformMoves());

describe("Move to Dash wallet", () => {
  it("moves everything under the quote's platformOp grant, and keeps the result while the sheet is closed", async () => {
    const { flow, vault, calls, answer } = setup();
    const settled = vi.fn();
    const stop = onMoveSettled(settled);
    flow.begin();
    expect([flow.open, flow.step]).toEqual([true, "confirm"]);
    const going = flow.move(GRANT, TO);
    await vi.waitFor(() => expect(flow.step).toBe("running"));
    expect(vault.calls.authorize[0]).toMatchObject({ purpose: { kind: "platformOp", maxDuffs: 0, maxCredits: "40000000000" }, walletId: WALLET_A });
    // One call at a time.
    await flow.move(GRANT, TO);
    expect(calls).toEqual([`move all ${TO} grant-1`]);
    flow.close();
    expect(flow.unseen).toBe(false);
    answer(OUTCOME);
    await going;
    await Promise.resolve();
    expect(settled).toHaveBeenCalledTimes(1);
    expect(flow.unseen).toBe(true);
    flow.begin();
    expect([flow.step, flow.outcome]).toEqual(["done", OUTCOME]);
    flow.close();
    flow.begin();
    expect([flow.step, flow.outcome]).toEqual(["confirm", null]);
    stop();
  });

  it("keeps a pending result, and its warning, after the sheet is dismissed, until Activity shows the move settled", async () => {
    const { flow, answer } = setup();
    const row = (state: PlatformActivity["state"]): PlatformActivity => ({ id: OUTCOME.id, kind: "moveToCore", state, credits: OUTCOME.credits, address: null, to: TO, expectedDuffs: OUTCOME.expectedDuffs, feeCredits: null, at: 1, mayLand: state === "pending", estimated: false });
    flow.begin();
    const going = flow.move(GRANT, TO);
    await vi.waitFor(() => expect(flow.step).toBe("running"));
    answer({ ...OUTCOME, state: "pending" });
    await going;
    // The sheet is open on the result: Activity settling it does not swap it for a new quote.
    const settled = [{ id: OUTCOME.id, kind: "moveToCore", state: "done", credits: OUTCOME.credits, address: null, to: TO, expectedDuffs: OUTCOME.expectedDuffs, at: 1 }] as PlatformActivity[];
    flow.settle(settled);
    expect([flow.open, flow.step, flow.pending]).toEqual([true, "done", true]);
    flow.close();
    flow.begin();
    flow.close();
    // Dismissed twice: it is still the pending result, not a fresh quote, and nothing to "show".
    expect([flow.step, flow.outcome?.state, flow.pending, flow.unseen]).toEqual(["done", "pending", true, false]);
    // Activity still has it pending, or does not list it yet: it stays.
    flow.settle([row("pending")]);
    flow.settle([]);
    flow.settle(null);
    expect(flow.pending).toBe(true);
    flow.settle([row("done")]);
    expect([flow.step, flow.outcome, flow.pending]).toEqual(["confirm", null, false]);
  });

  it("reads a pending move off Activity's rows", () => {
    const base = { id: "pm-1", credits: "1", address: null, to: null, expectedDuffs: null, feeCredits: null, at: 1, mayLand: false, estimated: false } as const;
    expect(hasPendingMove([{ ...base, kind: "moveToCore", state: "pending" }])).toBe(true);
    expect(hasPendingMove([{ ...base, kind: "moveToCore", state: "done" }, { ...base, kind: "incoming", state: "pending" }])).toBe(false);
    // A send shares the hold: a pending one holds a move too.
    expect(hasPendingMove([{ ...base, kind: "send", state: "pending" }])).toBe(true);
    expect(hasPendingMove(null)).toBe(false);
  });

  it("ignores an add from the Dash wallet: it holds neither a move nor a send", () => {
    const base = { id: "fund-1", credits: "1", address: "tdash1a", to: null, expectedDuffs: null, feeCredits: null, at: 1, mayLand: false, estimated: true } as const;
    expect(hasPendingMove([{ ...base, kind: "fromCore", state: "pending" }])).toBe(false);
    expect(hasPendingMove([{ ...base, kind: "fromCore", state: "pending", mayLand: true }])).toBe(false);
    expect(hasUnansweredOp([{ ...base, kind: "fromCore", state: "pending" }])).toBe(false);
    expect(sendHolds([{ ...base, kind: "fromCore", state: "pending" }])).toBe(false);
  });

  it("counts a failed move or send that may still land as holding, and one that never will as not", () => {
    const base = { id: "ps-1", credits: "1", address: null, to: null, expectedDuffs: null, feeCredits: null, at: 1, estimated: false } as const;
    expect(hasPendingMove([{ ...base, kind: "send", state: "failed", mayLand: true }])).toBe(true);
    expect(hasPendingMove([{ ...base, kind: "moveToCore", state: "failed", mayLand: true }])).toBe(true);
    expect(hasPendingMove([{ ...base, kind: "send", state: "failed", mayLand: false }])).toBe(false);
    expect(hasPendingMove([{ ...base, kind: "send", state: "replaced", mayLand: false }])).toBe(false);
    expect(hasPendingMove([{ ...base, kind: "incoming", state: "failed", mayLand: true }])).toBe(false);
    // Only a `pending` one is unanswered: a failed one that may land can still be retried.
    expect(hasUnansweredOp([{ ...base, kind: "send", state: "failed", mayLand: true }])).toBe(false);
    expect(hasUnansweredOp([{ ...base, kind: "send", state: "pending", mayLand: true }])).toBe(true);
  });

  it("the Move card waits only for an unanswered op: a held move (failed by height) leaves Move offered, a held send is named", () => {
    const base = { id: "x-1", credits: "1", address: null, to: null, expectedDuffs: null, feeCredits: null, at: 1, estimated: false } as const;
    const heldMove = { ...base, kind: "moveToCore", state: "failed", mayLand: true } as const;
    const heldSend = { ...base, kind: "send", state: "failed", mayLand: true } as const;
    // The card gates Move on hasUnansweredOp: a failed op that may land does not hide Move.
    expect([hasUnansweredOp([heldMove]), hasUnansweredOp([heldSend]), hasUnansweredOp([{ ...heldMove, state: "pending" }])]).toEqual([false, false, true]);
    expect([sendHolds([heldSend]), sendHolds([{ ...heldSend, state: "pending" }])]).toEqual([true, true]);
    expect([sendHolds([heldMove]), sendHolds([heldMove, heldSend]), sendHolds([{ ...heldSend, mayLand: false }]), sendHolds(null)]).toEqual([false, false, false, false]);
    expect(MOVE_COPY.waiting.sendHeld).toBe("A payment from Platform may still go through. Moving waits until it settles.");
  });

  it("names the held one in a refused move", () => {
    const base = { id: "x-1", credits: "1", address: null, to: null, expectedDuffs: null, feeCredits: null, at: 1, estimated: false } as const;
    const heldMove = { ...base, kind: "moveToCore", state: "failed", mayLand: true } as const;
    const heldSend = { ...base, kind: "send", state: "failed", mayLand: true } as const;
    const held = appError("credits.move_pending", { kind: "artifact", artifact: heldSend.id });
    expect(moveErrorText(held, credits)).toBe("An earlier payment or move from Platform may still go through. Wait for it to finish, then move again.");
    expect(moveErrorText(held, credits, [heldMove])).toBe("An earlier payment or move from Platform may still go through. Wait for it to finish, then move again.");
    // A held payment's own retry, in Pay, is the way out.
    expect(moveErrorText(held, credits, [heldSend])).toBe(
      "An earlier payment from Platform may still go through. Wait for it to finish, or send that same payment again from Pay and press Retry that payment, then move again.",
    );
  });

  it("a cancelled passphrase ran nothing: back at the quote, nothing read again", async () => {
    const { flow, api } = setup({ encrypted: true });
    const settled = vi.fn();
    const stop = onMoveSettled(settled);
    flow.begin();
    const going = flow.move(GRANT, TO);
    await vi.waitFor(() => expect(flow.grants.pending).not.toBeNull());
    flow.grants.cancel();
    await going;
    await Promise.resolve();
    expect([flow.step, api.platformMoveToCore.mock.calls.length, settled.mock.calls.length]).toEqual(["confirm", 0, 0]);
    stop();
  });

  it("a refusal keeps its error; Back only when a new quote may pass", async () => {
    const { flow, api } = setup();
    api.platformMoveToCore.mockRejectedValueOnce(appError("platform.grant_exceeded"));
    flow.begin();
    await flow.move(GRANT, TO);
    expect([flow.step, moveCanRetry(flow.error)]).toEqual(["refused", true]);
    flow.back();
    expect([flow.step, flow.error]).toEqual(["confirm", null]);
    api.platformMoveToCore.mockRejectedValueOnce(appError("platform.broadcast_unknown", { kind: "artifact", artifact: "x" }));
    await flow.move(GRANT, TO);
    // An unknown outcome is never offered again from here: the move could happen twice.
    expect([flow.step, moveCanRetry(flow.error)]).toEqual(["refused", false]);
  });

  it("names a refusal by its code", () => {
    expect(moveErrorText(appError("platform.insufficient_credits", { kind: "credits", needed: "2000000", available: "1000000" }), credits)).toContain("0.00002 tDASH needed with the fee, 0.00001 tDASH available");
    expect(moveErrorText(appError("platform.broadcast_unknown"), credits)).toContain("Check Activity");
    expect(moveErrorText(appError("platform.unavailable"), credits)).toBe("Dash Platform did not answer. Try again.");
    expect(moveErrorText(appError("platform.feature_off"), credits)).toContain("turned off");
    expect(moveErrorText(new Error("x"), credits)).toBe("Something went wrong.");
  });

  it("keeps one flow per wallet while the app runs", () => {
    const { api } = setup();
    expect(moveFor(api, WALLET_A)).toBe(moveFor(api, WALLET_A));
    expect(moveFor(api, "other")).not.toBe(moveFor(api, WALLET_A));
  });
});

describe("moveBlock", () => {
  it("holds Move back while Activity is unread or failed, and names a pending move", () => {
    expect(moveBlock(false, { isPending: true, isError: false })).toBe("checking");
    expect(moveBlock(false, { isPending: false, isError: true })).toBe("failed");
    expect(moveBlock(true, { isPending: false, isError: false })).toBe("pending");
    expect(moveBlock(false, { isPending: false, isError: false })).toBeNull();
  });
});

describe("invalidArgumentText", () => {
  it("shows the engine's detail as a sentence, from the reason or the message", () => {
    expect(invalidArgumentText({ message: "m", data: { kind: "reason", reason: "Consolidate coins first" } })).toBe("Consolidate coins first.");
    expect(invalidArgumentText({ message: "Too many coins.", data: null })).toBe("Too many coins.");
    expect(invalidArgumentText({ message: " ", data: null })).toBe("That was not accepted.");
  });
});

// The grant includes a fee reserve; Amount names only the planned withdrawal.
it("separates the planned withdrawal from its grant cap", async () => {
  const { plannedWithdrawalCredits } = await import("./move");
  expect(plannedWithdrawalCredits({ credits: "40000000000", feeCredits: "2000000" })).toBe("39998000000");
});
