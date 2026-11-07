// "Move mixed coins" to the wallet (move.ts) against the fixture's engine rules (fixture/coinjoin.ts):
// the plan's mixed split, the refusal while mixing and Stop Mixing, the spend grant, and the
// complete, partial and unknown outcomes.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import { IpcBackend } from "../api/tauri";
import { installFixture } from "../fixture/index";
import { FIXTURE_PASSPHRASE, FIXTURE_WALLET_ID as W, type ScenarioName } from "../fixture/scenarios";
import { appError } from "../onboarding/testWorld";
import type { MixedCoinsSweepPlan } from "../api/generated/MixedCoinsSweepPlan";
import type { CoinJoinStatus } from "../api/generated/CoinJoinStatus";
import { canRetry, MoveMixedFlow, type MoveMixedApi, moveErrorText, MOVE_COPY, planFee, planReceives, resetMoveMixed, resultText, showsMoveCard } from "./move";

const until = async (cond: () => boolean) => {
  for (let i = 0; i < 200 && !cond(); i++) await new Promise((r) => setTimeout(r, 0));
  expect(cond()).toBe(true);
};

let flow: MoveMixedFlow;
const open = async (scenario: ScenarioName) => {
  installFixture({ scenario });
  flow = new MoveMixedFlow(new IpcBackend("fixture"), W);
  flow.begin();
  await until(() => flow.step !== "plan");
};

/** Move, answering the passphrase prompt if the vault asks for one. */
const move = async () => {
  const done = flow.move();
  await until(() => flow.grants.pending !== null || !flow.running);
  if (flow.grants.pending) expect(await flow.grants.submit(FIXTURE_PASSPHRASE)).toBe(true);
  await done;
};

beforeEach(() => vi.stubGlobal("window", globalThis));
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

const amount = (d: number) => `${d} duffs`;

describe("Move mixed coins to the wallet", () => {
  it("plans the whole CoinJoin balance, fully mixed and not, then moves it under a spend grant", async () => {
    await open("coinjoin-move");
    expect(flow.step).toBe("confirm");
    const plan = flow.plan!;
    // Two of the three coins have the network's rounds, one is short of them.
    expect([plan.fullyMixed, plan.notFullyMixed]).toEqual([2 * 10_000_100, 10_000_100]);
    expect(plan.total).toBe(plan.fullyMixed + plan.notFullyMixed);
    expect(plan.chunks).toHaveLength(1);
    expect(planReceives(plan) + planFee(plan)).toBe(plan.total);

    await move();
    expect(flow.step).toBe("done");
    const r = flow.result!;
    expect(r).toMatchObject({ moved: plan.total, received: planReceives(plan), fee: planFee(plan), remaining: 0, failureCode: null, unknownTxid: null });
    // What arrives, the fee apart (R5).
    expect(resultText(r, amount)).toEqual({ kind: "done", title: MOVE_COPY.done.title, text: MOVE_COPY.done.text(amount(planReceives(plan)), amount(planFee(plan))) });
    // The CoinJoin account is empty: a new plan has nothing to move.
    await expect(cmd.mixedCoinsSweepPlan(W, "wallet")).rejects.toMatchObject({ code: "coinjoin.nothing_to_move" });
  });

  it("is refused while the wallet mixes, and offers Stop Mixing", async () => {
    await open("coinjoin-move-mixing");
    expect(flow.step).toBe("mixing");
    expect(moveErrorText(flow.error)).toBe(MOVE_COPY.mixing.text);
    await expect(cmd.moveMixedCoins(W, "wallet", "no-quote", "no-grant")).rejects.toMatchObject({ code: "coinjoin.mixing_active" });

    await flow.stopMixing();
    await until(() => flow.step !== "plan");
    expect((await cmd.coinjoinStatus(W)).state).toBe("idle");
    expect(flow.step).toBe("confirm");
  });

  it("asks for a grant covering the plan's total", async () => {
    await open("coinjoin-move");
    const small = await cmd.authorize({ kind: "spend", maxDuffs: flow.plan!.total - 1 }, W, { kind: "passphrase", passphrase: FIXTURE_PASSPHRASE });
    await expect(cmd.moveMixedCoins(W, "wallet", flow.plan!.quoteId, small.id)).rejects.toMatchObject({ code: "coinjoin.grant_invalid" });
  });

  it("reports a partial move: what moved and what stayed", async () => {
    await open("coinjoin-move-partial");
    expect(flow.plan!.chunks.map((c) => c.inputs)).toEqual([500, 100]);
    await move();
    const r = flow.result!;
    expect(r.failureCode).toBe("coinjoin.broadcast_rejected");
    expect(r.txids).toHaveLength(1);
    expect(r.moved + r.remaining).toBe(flow.plan!.total);
    expect(r.received + r.fee).toBe(r.moved);
    expect(resultText(r, amount)).toMatchObject({ kind: "partial", text: MOVE_COPY.partial.text(amount(r.received), amount(r.fee), amount(r.remaining)) });

    // Try again plans what stayed.
    flow.again();
    await until(() => flow.step !== "plan");
    expect(flow.plan!.chunks.map((c) => c.inputs)).toEqual([100]);
  });

  it("names a chunk whose broadcast outcome is unknown, its coins held", async () => {
    await open("coinjoin-move-unknown");
    await move();
    const r = flow.result!;
    expect(r).toMatchObject({ failureCode: "coinjoin.broadcast_unknown", moved: 0 });
    expect(r.unknownTxid).not.toBeNull();
    expect(resultText(r, amount)).toMatchObject({ kind: "unknown", text: MOVE_COPY.unknown });
    // Its coins stay reserved: nothing left to plan.
    await expect(cmd.mixedCoinsSweepPlan(W, "wallet")).rejects.toMatchObject({ code: "coinjoin.nothing_to_move" });
  });

  it("Try again on an unknown chunk sends its recorded transaction again: no grant, no new move", async () => {
    installFixture({ scenario: "coinjoin-move-unknown" });
    const api = new IpcBackend("fixture");
    flow = new MoveMixedFlow(api, W);
    flow.begin();
    await until(() => flow.step !== "plan");
    await move();
    const txid = flow.result!.unknownTxid!;
    const resend = vi.spyOn(api, "resendMixedCoins");
    const moveAgain = vi.spyOn(api, "moveMixedCoins");
    const authorize = vi.spyOn(api, "authorize");
    await flow.resend();
    expect(resend).toHaveBeenCalledWith(W, txid);
    expect(moveAgain).not.toHaveBeenCalled();
    expect(authorize).not.toHaveBeenCalled();
    expect(flow).toMatchObject({ step: "done", resent: "unknown" });
    expect(MOVE_COPY.resent.unknown).toContain("reserved");
    // A chunk the wallet no longer records resolved earlier.
    await expect(cmd.resendMixedCoins(W, "ab".repeat(32))).resolves.toEqual([{ txid: "ab".repeat(32), state: "resolved" }]);
  });

  it("a move whose coins changed after the confirm returns to the confirm with the new plan", async () => {
    installFixture({ scenario: "coinjoin-move" });
    const api = new IpcBackend("fixture");
    flow = new MoveMixedFlow(api, W);
    flow.begin();
    await until(() => flow.step !== "plan");
    const confirmed = flow.plan!;
    // Another plan (another sheet, or the coins changing) replaces the confirmed quote.
    await api.mixedCoinsSweepPlan(W, "wallet");
    await expect(api.moveMixedCoins(W, "wallet", confirmed.quoteId, "no-grant")).rejects.toMatchObject({ code: "coinjoin.plan_changed" });
    await api.mixedCoinsSweepPlan(W, "wallet");
    const moveSpy = vi.spyOn(api, "moveMixedCoins").mockRejectedValueOnce(appError("coinjoin.plan_changed"));
    await move();
    expect(moveSpy).toHaveBeenCalledTimes(1);
    expect(flow).toMatchObject({ step: "confirm", replanned: true });
    expect(flow.plan!.quoteId).not.toBe(confirmed.quoteId);
  });

  it("keeps a running move's result for the reopened sheet", async () => {
    await open("coinjoin-move");
    const done = move();
    flow.close();
    await done;
    expect(flow).toMatchObject({ open: false, step: "done" });
    flow.begin();
    expect(flow).toMatchObject({ open: true, step: "done" });
    // Closing a seen result drops it: the next open plans again.
    flow.close();
    expect(flow.step).toBe("plan");
  });

  it("a cancelled passphrase moves nothing: back at the confirm", async () => {
    await open("coinjoin-move");
    const going = flow.move();
    await until(() => flow.grants.pending !== null);
    flow.grants.cancel();
    await going;
    expect(flow.step).toBe("confirm");
    expect((await cmd.mixedCoinsSweepPlan(W, "wallet")).total).toBe(flow.plan!.total);
  });

  it("leaves the shielded destination to the shielded flow", async () => {
    installFixture({ scenario: "coinjoin-move" });
    await expect(cmd.mixedCoinsSweepPlan(W, "shielded")).rejects.toMatchObject({ code: "platform.not_implemented" });
  });
});

describe("Move mixed coins, the flow's edges", () => {
  const PLAN: MixedCoinsSweepPlan = { destination: "wallet", total: 30, chunks: [{ inputs: 3, amount: 27, fee: 3 }], fullyMixed: 20, notFullyMixed: 10, unconfirmed: 0, quoteId: "q" };
  const stub = (over: Partial<Record<keyof MoveMixedApi, unknown>>) => new MoveMixedFlow({ stopMixing: vi.fn().mockResolvedValue(undefined), ...over } as unknown as MoveMixedApi, W);

  it("after Stop Mixing, plans again while the last session winds down", async () => {
    const mixing = appError("coinjoin.mixing_active");
    const plan = vi.fn().mockRejectedValueOnce(mixing).mockRejectedValueOnce(mixing).mockRejectedValueOnce(mixing).mockResolvedValue(PLAN);
    const f = stub({ mixedCoinsSweepPlan: plan });
    f.begin();
    await vi.waitFor(() => expect(f.step).toBe("mixing"));
    await f.stopMixing();
    expect([f.step, plan.mock.calls.length]).toEqual(["confirm", 4]);
  });

  it("a stale approval offers Try again, which plans afresh", async () => {
    const plan = vi.fn().mockResolvedValue(PLAN);
    const f = stub({ mixedCoinsSweepPlan: plan, moveMixedCoins: vi.fn().mockRejectedValue(appError("coinjoin.grant_invalid")) });
    f.grants.withGrant = (async (_p: unknown, _w: unknown, run: (g: { id: string }) => Promise<unknown>) => ({ value: await run({ id: "g" }) })) as typeof f.grants.withGrant;
    f.begin();
    await vi.waitFor(() => expect(f.step).toBe("confirm"));
    await f.move();
    expect([f.step, moveErrorText(f.error), canRetry(f.error)]).toEqual(["refused", MOVE_COPY.changed, true]);
    expect(canRetry(appError("coinjoin.nothing_to_move"))).toBe(false);
    f.again();
    await vi.waitFor(() => expect(f.step).toBe("confirm"));
    expect(plan).toHaveBeenCalledTimes(2);
  });

  it("a plan refusal that came while the sheet was closed is not kept: reopening plans again", async () => {
    const plan = vi.fn().mockRejectedValueOnce(appError("coinjoin.mixing_active")).mockRejectedValueOnce(appError("coinjoin.nothing_to_move")).mockResolvedValue(PLAN);
    let stopped!: () => void;
    const f = stub({ mixedCoinsSweepPlan: plan, stopMixing: vi.fn(() => new Promise<void>((ok) => (stopped = ok))) });
    f.begin();
    await vi.waitFor(() => expect(f.step).toBe("mixing"));
    const stopping = f.stopMixing();
    f.close();
    stopped();
    await stopping;
    expect(f.step).toBe("refused");
    f.begin();
    await vi.waitFor(() => expect(f.step).toBe("confirm"));
  });

  it("a refusal that came while the sheet was closed waits for it", async () => {
    let refuse!: (e: unknown) => void;
    const plan = vi.fn().mockResolvedValueOnce(PLAN).mockResolvedValue(PLAN);
    const f = stub({ mixedCoinsSweepPlan: plan, moveMixedCoins: vi.fn(() => new Promise((_, no) => (refuse = no))) });
    f.grants.withGrant = (async (_p: unknown, _w: unknown, run: (g: { id: string }) => Promise<unknown>) => ({ value: await run({ id: "g" }) })) as typeof f.grants.withGrant;
    f.begin();
    await vi.waitFor(() => expect(f.step).toBe("confirm"));
    const going = f.move();
    await vi.waitFor(() => expect(f.step).toBe("running"));
    f.close();
    refuse(appError("coinjoin.vault_locked"));
    await going;
    f.begin();
    expect([f.open, f.step, plan.mock.calls.length]).toEqual([true, "refused", 1]);
    f.close();
    expect(f.step).toBe("plan");
  });
});

// The card follows the CoinJoin account's free coins, not its denominations.
describe("the move card", () => {
  const status = (coinjoinAccount: number, extra: Partial<CoinJoinStatus["balances"]> = {}) =>
    ({ unavailable: null, balances: { anonymizable: 0, denominated: 0, normalizedAnonymized: 0, fullyMixed: 0, coinjoinAccount, ...extra } }) as unknown as CoinJoinStatus;
  const idle = { open: false, running: false, step: "plan" as const };

  it("shows for a collateral alone, a consolidated output, and after a restart", () => {
    expect(showsMoveCard(status(40_000), idle)).toBe(true);
    expect(showsMoveCard(status(3_000_000_000), idle)).toBe(true);
    // A fresh app: no flow kept, the account's coins are what counts.
    resetMoveMixed();
    expect(showsMoveCard(status(3_000_000_000), new MoveMixedFlow({} as MoveMixedApi, W))).toBe(true);
  });

  it("hides when the account holds nothing free, unless a move runs or its result waits", () => {
    expect(showsMoveCard(status(0, { denominated: 0 }), idle)).toBe(false);
    expect(showsMoveCard(status(0), { ...idle, step: "done" })).toBe(true);
    expect(showsMoveCard({ ...status(40_000), unavailable: { kind: "watchOnly" } } as CoinJoinStatus, idle)).toBe(false);
  });
});
