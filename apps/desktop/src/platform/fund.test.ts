// The "Add from Dash wallet" flow: it runs under the quote's duffs grant, a lock that was not applied
// is finished with one duff of grant, the sheet can close while the call runs, and the refusals have
// their words.

import { afterEach, describe, expect, it, vi } from "vitest";
import type { PlatformFundOutcome } from "../api/generated/PlatformFundOutcome";
import { appError, WALLET_A } from "../onboarding/testWorld";
import { fakeVault } from "../tools/testApi";
import { fundCanChange, fundErrorText, fundFor, PlatformFundFlow, resetPlatformFunds } from "./fund";
import { onMoveSettled } from "./move";

const APPLIED: PlatformFundOutcome = { txid: "lock1", state: { kind: "applied", credits: "49944000000" } };
const GRANT = { maxDuffs: 50_001_000, maxCredits: "0" };
const credits = (c: string) => `${Number(BigInt(c) / 1000n) / 1e8} tDASH`;
const duffs = (d: number) => `${d / 1e8} tDASH`;
const text = (e: unknown) => fundErrorText(e, credits, duffs);

function setup(lock = { encrypted: false }) {
  const vault = fakeVault(lock);
  const calls: string[] = [];
  let answer: ((o: PlatformFundOutcome) => void) | null = null;
  const api = {
    ...vault.api,
    platformFund: vi.fn(
      (_w: string, d: number, grant: string) =>
        new Promise<PlatformFundOutcome>((resolve) => {
          calls.push(`fund ${d} ${grant}`);
          answer = resolve;
        }),
    ),
    resumePlatformFund: vi.fn(async (_w: string, txid: string, grant: string) => (calls.push(`resume ${txid} ${grant}`), APPLIED)),
  };
  const flow = new PlatformFundFlow(api, WALLET_A);
  return { flow, api, vault, calls, answer: (o: PlatformFundOutcome) => answer!(o) };
}

afterEach(() => resetPlatformFunds());

describe("the Add from Dash wallet flow", () => {
  it("adds under a duffs grant, can close while it runs, and reads again", async () => {
    const { flow, vault, calls, answer } = setup();
    const settled = vi.fn();
    const stop = onMoveSettled(settled);
    flow.begin();
    flow.next(50_000_000);
    const going = flow.run(GRANT);
    await vi.waitFor(() => expect(flow.step).toBe("running"));
    expect(vault.calls.authorize[0]).toMatchObject({ purpose: { kind: "platformOp", maxDuffs: 50_001_000, maxCredits: "0" }, walletId: WALLET_A });
    flow.close();
    await flow.run(GRANT);
    expect(calls).toEqual(["fund 50000000 grant-1"]);
    expect(flow.unseen).toBe(false);
    answer(APPLIED);
    await going;
    expect([flow.unseen, flow.step, flow.outcome]).toEqual([true, "done", APPLIED]);
    flow.begin();
    expect(flow.step).toBe("done");
    flow.close();
    expect([flow.unseen, flow.step]).toEqual([false, "form"]);
    expect(settled).toHaveBeenCalledTimes(1);
    stop();
  });

  it("finishes a lock that was not applied with one duff of grant and no confirm step", async () => {
    const { flow, vault, calls } = setup();
    await flow.resume("lock1");
    expect(vault.calls.authorize[0]).toMatchObject({ purpose: { kind: "platformOp", maxDuffs: 1, maxCredits: "0" } });
    expect(calls).toEqual(["resume lock1 grant-1"]);
    expect([flow.open, flow.step, flow.outcome]).toEqual([true, "done", APPLIED]);
  });

  it("a cancelled passphrase ran nothing: back at the confirm step, and nothing is read again", async () => {
    const { flow, api } = setup({ encrypted: true });
    const settled = vi.fn();
    const stop = onMoveSettled(settled);
    flow.begin();
    flow.next(50_000_000);
    const going = flow.run(GRANT);
    await vi.waitFor(() => expect(flow.grants.pending).not.toBeNull());
    flow.grants.cancel();
    await going;
    expect([flow.step, api.platformFund.mock.calls.length, settled.mock.calls.length]).toEqual(["confirm", 0, 0]);
    stop();
  });

  it("a cancelled Finish passphrase closes the sheet instead of leaving an empty form", async () => {
    const { flow, api } = setup({ encrypted: true });
    const going = flow.resume("lock1");
    await vi.waitFor(() => expect(flow.grants.pending).not.toBeNull());
    flow.grants.cancel();
    await going;
    expect([flow.open, flow.step, api.resumePlatformFund.mock.calls.length]).toEqual([false, "form", 0]);
  });

  it("a refusal keeps its error, reads again, and offers Back when another amount may pass", async () => {
    const { flow, api } = setup();
    const settled = vi.fn();
    const stop = onMoveSettled(settled);
    api.platformFund.mockRejectedValueOnce(appError("credits.below_minimum", { kind: "number", value: 56_001 }));
    flow.begin();
    flow.next(10);
    await flow.run(GRANT);
    expect([flow.step, fundCanChange(flow.error)]).toEqual(["refused", true]);
    expect(settled).toHaveBeenCalledTimes(1);
    flow.back();
    expect([flow.step, flow.error]).toEqual(["form", null]);
    flow.next(50_000_000);
    api.platformFund.mockRejectedValueOnce(appError("platform.broadcast_unknown", { kind: "artifact", artifact: "x" }));
    await flow.run(GRANT);
    expect([flow.step, fundCanChange(flow.error)]).toEqual(["refused", false]);
    stop();
  });

  it("keeps one flow per wallet", () => {
    const { api } = setup();
    expect(fundFor(api, WALLET_A)).toBe(fundFor(api, WALLET_A));
    expect(fundFor(api, "other")).not.toBe(fundFor(api, WALLET_A));
  });
});

describe("the Add from Dash wallet error texts", () => {
  it("names the minimum and what is short", () => {
    expect(text(appError("credits.below_minimum", { kind: "number", value: 56_001 }))).toBe("Too small to add: the smallest amount is 0.00056001 tDASH.");
    expect(text(appError("credits.funding_insufficient", { kind: "duffs", needed: 200_000_000, available: 100_000_000 }))).toBe("Not enough Dash in your wallet: 2 tDASH needed with the fee, 1 tDASH available.");
  });

  it("has words for the too-many-coins and unfinished-add refusals", () => {
    expect(text(appError("credits.too_many_inputs", { kind: "number", value: 700 }))).toBe("That takes too many separate coins at once. Consolidate your coins first, then try again.");
    expect(text(appError("credits.fund_pending", { kind: "txid", txid: "ab" }))).toBe("An earlier add to Platform is not finished. Finish it from the Platform card first.");
    expect(text(appError("platform.feature_off", { kind: "feature", feature: "platform" }))).toBe("Platform addresses are not active on this network yet.");
  });

  it("offers Back for amount-related refusals only", () => {
    for (const code of ["credits.below_minimum", "credits.funding_insufficient", "platform.grant_exceeded", "credits.too_many_inputs"] as const) expect(fundCanChange(appError(code))).toBe(true);
    for (const code of ["credits.fund_pending", "platform.feature_off", "credits.coins_locked"] as const) expect(fundCanChange(appError(code))).toBe(false);
  });
});
