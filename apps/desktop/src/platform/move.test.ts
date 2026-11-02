// "Move to Dash wallet": everything moves (`all`) under the quote's platformOp grant; the sheet
// can close while the call runs and reopens to its result; refusals say what to do.

import { afterEach, describe, expect, it, vi } from "vitest";
import type { MoveToCoreOutcome } from "../api/generated/MoveToCoreOutcome";
import type { PlatformActivity } from "../api/generated/PlatformActivity";
import { appError, WALLET_A } from "../onboarding/testWorld";
import { fakeVault } from "../tools/testApi";
import { hasPendingMove, moveBlock, moveCanRetry, moveErrorText, moveFor, onMoveSettled, PlatformMoveFlow, resetPlatformMoves } from "./move";

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
    const row = (state: PlatformActivity["state"]): PlatformActivity => ({ id: OUTCOME.id, kind: "moveToCore", state, credits: OUTCOME.credits, address: null, to: TO, expectedDuffs: OUTCOME.expectedDuffs, feeCredits: null, at: 1 });
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
    const base = { id: "pm-1", credits: "1", address: null, to: null, expectedDuffs: null, feeCredits: null, at: 1 } as const;
    expect(hasPendingMove([{ ...base, kind: "moveToCore", state: "pending" }])).toBe(true);
    expect(hasPendingMove([{ ...base, kind: "moveToCore", state: "done" }, { ...base, kind: "incoming", state: "pending" }])).toBe(false);
    expect(hasPendingMove(null)).toBe(false);
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
