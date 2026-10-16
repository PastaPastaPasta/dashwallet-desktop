// The top-up flow (DP1-10), by parity ID: IOS-069 (the top-up sheet: DASHPAY F8's IOS-131
// presets, confirm, outcome; Android's AND-078 confirm copy; pending top-ups: Finish, "Credits
// arrived").

import { afterEach, describe, expect, it, vi } from "vitest";
import type { TopUpOutcome } from "../api/generated/TopUpOutcome";
import { appError, WALLET_A } from "../onboarding/testWorld";
import { fakeVault } from "../tools/testApi";
import { amountCanChange, FINISH_GRANT, minTopUp, onTopUpSettled, outcomeText, resetTopUps, routesToRepair, topUpCredits, topUpErrorText, topUpFor, TopUpFlow } from "./topup";

const amount = (duffs: number) => `${duffs / 1e8} tDASH`;

function setup(lock = { encrypted: false }) {
  const vault = fakeVault(lock);
  const calls: string[] = [];
  let resolveTopUp: ((o: TopUpOutcome) => void) | null = null;
  const api = {
    ...vault.api,
    creditsTopUp: vi.fn(
      (_walletId: string, identity: string, duffs: number, grant: string) =>
        new Promise<TopUpOutcome>((resolve) => {
          calls.push(`topUp ${identity} ${duffs} ${grant}`);
          resolveTopUp = resolve;
        }),
    ),
    creditsResumeTopUp: vi.fn(async (_walletId: string, txid: string, grant: string): Promise<TopUpOutcome> => {
      calls.push(`resume ${txid} ${grant}`);
      return { txid, state: { kind: "applied", creditsAdded: "5", balance: "10" } };
    }),
    creditsResolveTopUp: vi.fn(async (_walletId: string, txid: string) => (calls.push(`resolve ${txid}`), null)),
  };
  const flow = new TopUpFlow(api, WALLET_A);
  return { flow, api, vault, calls, finishTopUp: (o: TopUpOutcome) => resolveTopUp!(o) };
}

const applied: TopUpOutcome = { txid: "aa", state: { kind: "applied", creditsAdded: "4950000000", balance: "189450000000" } };
const pending = { txid: "bb", identity: "A", identityIndex: 0, duffs: 5_000_000, proved: true, consumptionUnknown: false };

afterEach(() => resetTopUps());

describe("the top-up sheet (IOS-069, AND-078)", () => {
  it("D5: the presets and the floor; what a top-up buys", () => {
    expect(minTopUp(undefined)).toBe(1_000_000);
    expect(minTopUp({ topUpMinDuffs: 2_000_000 })).toBe(2_000_000);
    expect(topUpCredits(5_000_000, { creditsPerDuff: 1000, topUpFee: "50000000" })).toBe(4_950_000_000n);
    expect(topUpCredits(10, { creditsPerDuff: 1000, topUpFee: "50000000" })).toBe(0n);
  });

  it("T1 → T2 → T3 → T4: pays under the quote's platformOp grant; one call at a time (D14)", async () => {
    const { flow, vault, calls, finishTopUp } = setup();
    flow.begin("A");
    expect([flow.open, flow.step]).toEqual([true, "amount"]);
    flow.next(5_000_000);
    expect(flow.step).toBe("confirm");
    const paying = flow.pay({ maxDuffs: 5_002_260, maxCredits: "0" });
    await vi.waitFor(() => expect(flow.step).toBe("running"));
    expect(vault.calls.authorize[0]).toMatchObject({ purpose: { kind: "platformOp", maxDuffs: 5_002_260, maxCredits: "0" }, walletId: WALLET_A });
    // A second Confirm while it runs does nothing.
    await flow.pay({ maxDuffs: 5_002_260, maxCredits: "0" });
    expect(calls).toEqual(["topUp A 5000000 grant-1"]);
    // Closed while it runs, reopened to its result, which the page offers meanwhile.
    flow.close();
    expect([flow.open, flow.unseen]).toEqual([false, false]);
    finishTopUp(applied);
    await paying;
    expect(flow.unseen).toBe(true);
    flow.begin("A");
    expect(flow.unseen).toBe(false);
    expect([flow.open, flow.step, flow.outcome]).toEqual([true, "done", applied]);
    flow.close();
    flow.begin("A");
    expect(flow.step).toBe("amount");
  });

  it("T8: a refusal keeps its error; Back when another amount may pass, Repair for an unknown funding", async () => {
    const { flow, api } = setup();
    api.creditsTopUp.mockRejectedValueOnce(appError("platform.broadcast_unknown", { kind: "artifact", artifact: "topup/0-1/funding" }));
    flow.begin("A");
    flow.next(5_000_000);
    await flow.pay({ maxDuffs: 1, maxCredits: "0" });
    expect(flow.step).toBe("refused");
    expect(routesToRepair(flow.error)).toBe(true);
    expect(amountCanChange(flow.error)).toBe(false);
    expect(amountCanChange(appError("credits.funding_insufficient", { kind: "duffs", needed: 2, available: 1 }))).toBe(true);
    flow.back();
    expect([flow.step, flow.error]).toEqual(["amount", null]);
  });

  it("a cancelled passphrase returns to the confirm step and pays nothing", async () => {
    const { flow, calls } = setup({ encrypted: true });
    const settled = vi.fn();
    const stop = onTopUpSettled(settled);
    flow.begin("A");
    flow.next(5_000_000);
    const paying = flow.pay({ maxDuffs: 1, maxCredits: "0" });
    await vi.waitFor(() => expect(flow.grants.pending).not.toBeNull());
    flow.grants.cancel();
    await paying;
    await Promise.resolve();
    expect([flow.step, calls]).toEqual(["confirm", []]);
    // Nothing ran, so nothing is read again.
    expect(settled).not.toHaveBeenCalled();
    stop();
  });

  it("T4–T7: the outcome screens", () => {
    const balance = (c: string) => `${c} cr`;
    expect(outcomeText(applied, balance)).toEqual({ title: "Credits added", text: "+4,950,000,000 credits. Your balance is 189450000000 cr." });
    expect(outcomeText({ txid: "a", state: { kind: "proofWaiting" } }, balance).title).toBe("Payment sent");
    expect(outcomeText({ txid: "a", state: { kind: "notApplied", error: "x" } }, balance).title).toBe("The credits were not added yet");
    expect(outcomeText({ txid: "a", state: { kind: "consumptionUnknown", balance: null } }, balance).title).toBe("Check your credit balance");
  });

  it("D17: refusal copy by code, DP3-04's gate codes included; discreet hides what is available", () => {
    const insufficient = appError("credits.funding_insufficient", { kind: "duffs", needed: 5_002_260, available: 1_000_000 });
    expect(topUpErrorText(insufficient, amount)).toBe("Not enough Dash: 0.0500226 tDASH needed, 0.01 tDASH available.");
    expect(topUpErrorText(insufficient, amount, true)).toBe("Not enough Dash: 0.0500226 tDASH needed.");
    expect(topUpErrorText(appError("credits.below_minimum", { kind: "limit", duffs: 1_000_000 }), amount)).toBe("The smallest top-up is 0.01 tDASH.");
    expect(topUpErrorText(appError("platform.context_unavailable"), amount)).toBe("Waiting for the network to sync and verify this identity. Try again shortly.");
    expect(topUpErrorText(appError("platform.trust_mismatch"), amount)).toBe("This identity's data could not be verified against the network. Nothing was paid.");
    expect(topUpErrorText(appError("platform.will_be_sent"), amount)).toBe("The payment is still being sent. Try again in a minute.");
  });

  it("D14: an unseen result is dropped once a row's call or Repair overtakes it", async () => {
    const { flow, finishTopUp } = setup();
    flow.begin("A");
    flow.next(5_000_000);
    const paying = flow.pay({ maxDuffs: 5_002_260, maxCredits: "0" });
    await vi.waitFor(() => expect(flow.step).toBe("running"));
    flow.close();
    finishTopUp({ txid: "bb", state: { kind: "proofWaiting" } });
    await paying;
    expect(flow.unseen).toBe(true);
    await flow.finish(pending);
    expect(flow.unseen).toBe(false);
    flow.begin("A");
    expect(flow.step).toBe("amount");
    flow.close();
    // Repair's resolution (`forget`) drops one too; an open sheet keeps what it shows.
    flow.step = "refused";
    flow.forget();
    expect(flow.step).toBe("amount");
    flow.open = true;
    flow.step = "done";
    flow.forget();
    expect(flow.step).toBe("done");
  });

  it("D22: every call that ends tells the settle listeners", async () => {
    const { flow, finishTopUp } = setup();
    const settled = vi.fn();
    const stop = onTopUpSettled(settled);
    flow.begin("A");
    flow.next(5_000_000);
    const paying = flow.pay({ maxDuffs: 5_002_260, maxCredits: "0" });
    await vi.waitFor(() => expect(flow.step).toBe("running"));
    expect(settled).not.toHaveBeenCalled();
    finishTopUp(applied);
    await paying;
    await flow.finish(pending);
    await vi.waitFor(() => expect(settled).toHaveBeenCalledTimes(2));
    stop();
    await flow.finish(pending);
    await Promise.resolve();
    expect(settled).toHaveBeenCalledTimes(2);
  });

  it("D14: one flow per wallet, kept while the app runs", () => {
    const { api } = setup();
    expect(topUpFor(api, WALLET_A)).toBe(topUpFor(api, WALLET_A));
    expect(topUpFor(api, WALLET_A)).not.toBe(topUpFor(api, "other"));
  });
});

describe("pending top-ups (IOS-069, D15)", () => {
  it("Finish resumes under a one-duff funding grant; Credits arrived resolves a proved lock only", async () => {
    const { flow, vault, calls } = setup();
    await flow.finish(pending);
    expect(vault.calls.authorize[0].purpose).toEqual({ kind: "platformOp", ...FINISH_GRANT });
    expect(flow.rowResult).toMatchObject({ txid: "bb", error: null, outcome: { state: { kind: "applied" } } });
    await flow.creditsArrived({ ...pending, proved: false });
    await flow.creditsArrived(pending);
    expect(calls).toEqual(["resume bb grant-1", "resolve bb"]);
  });

  it("D15: a call answered consumptionUnknown is remembered until Credits arrived resolves it", async () => {
    const { flow, api } = setup();
    api.creditsResumeTopUp.mockResolvedValueOnce({ txid: "bb", state: { kind: "consumptionUnknown", balance: null } });
    await flow.finish(pending);
    expect(flow.answeredUnknown.has("bb")).toBe(true);
    await flow.creditsArrived(pending);
    expect(flow.answeredUnknown.has("bb")).toBe(false);
  });

  it("a row's refusal shows under the row", async () => {
    const { flow, api } = setup();
    api.creditsResolveTopUp.mockRejectedValueOnce(appError("invalid_argument"));
    await flow.creditsArrived(pending);
    expect(flow.rowResult).toMatchObject({ txid: "bb", outcome: null });
    expect(flow.rowRunning).toBeNull();
  });
});
