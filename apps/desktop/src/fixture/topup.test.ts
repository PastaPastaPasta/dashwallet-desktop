// The fixture's credits top-up and contest read (DP1-10) over the real IPC path: the quote's
// refusals in the engine's order, a top-up applied or left pending, Finish and "Credits arrived",
// a stuck funding that Repair resolves, `contest_status`, and DP6-02's withdrawals.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import { IDENTITY } from "./dashpay";
import { installFixture } from "./index";
import { FIXTURE_WALLET_ID, type ScenarioName } from "./scenarios";
import { TOP_UP_FEE_DUFFS, topUpStep, WITHDRAW_RULES, withdrawalId } from "./topup";

const W = FIXTURE_WALLET_ID;
const A = IDENTITY.alice;

beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  vi.stubGlobal("sessionStorage", { getItem: () => null, setItem: () => undefined, removeItem: () => undefined });
  vi.stubGlobal("location", { reload: () => undefined });
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

const install = (scenario: ScenarioName) => installFixture({ scenario });

async function grant(kind: "platformOp" | "spend", maxDuffs: number) {
  const purpose = kind === "spend" ? ({ kind, maxDuffs } as const) : ({ kind, maxDuffs, maxCredits: "0" } as const);
  return (await cmd.authorize(purpose, W, { kind: "passphrase", passphrase: "demo" })).id;
}

const code = (p: Promise<unknown>) => p.then(() => null, (e: { code: string }) => e.code);

describe("credits top-up (DP1-06's calls, DP1-10)", () => {
  it("quotes, applies under the quote's grant, and adds the credits", async () => {
    install("dashpay-registered");
    const quote = await cmd.creditsTopUpQuote(W, A, 5_000_000);
    expect(quote).toEqual({ feeDuffs: TOP_UP_FEE_DUFFS, totalDuffs: 5_000_000 + TOP_UP_FEE_DUFFS, credits: "4950000000", grant: { maxDuffs: 5_002_260, maxCredits: "0" } });
    expect(await code(cmd.creditsTopUp(W, A, 5_000_000, await grant("platformOp", 1)))).toBe("platform.grant_exceeded");
    const outcome = await cmd.creditsTopUp(W, A, 5_000_000, await grant("platformOp", quote.totalDuffs));
    expect(outcome.state).toEqual({ kind: "applied", creditsAdded: "4950000000", balance: "189450000000" });
    expect((await cmd.identities(W))[0].balance).toBe("189450000000");
    expect(await cmd.creditsPendingTopUps(W)).toEqual([]);
  });

  it("refuses below the minimum and past the wallet's coins", async () => {
    install("dashpay-registered");
    expect(await code(cmd.creditsTopUpQuote(W, A, 10))).toBe("credits.below_minimum");
    expect(await code(cmd.creditsTopUpQuote(W, A, 1e15))).toBe("credits.funding_insufficient");
  });

  it("a pending lock refuses the next top-up; Finish applies it and Credits arrived needs a proof", async () => {
    install("dashpay-top-up-pending");
    const [row] = await cmd.creditsPendingTopUps(W);
    expect(await code(cmd.creditsTopUpQuote(W, A, 5_000_000))).toBe("credits.top_up_pending");
    expect(await code(cmd.creditsResolveTopUp(W, row.txid))).toBe("invalid_argument");
    const outcome = await cmd.creditsResumeTopUp(W, row.txid, await grant("platformOp", 1));
    expect(outcome.state.kind).toBe("applied");
    expect(await cmd.creditsPendingTopUps(W)).toEqual([]);
  });

  it("a proved lock with its consumption unknown resolves without a grant", async () => {
    install("dashpay-top-up-unknown");
    const [row] = await cmd.creditsPendingTopUps(W);
    await cmd.creditsResolveTopUp(W, row.txid);
    expect(await cmd.creditsPendingTopUps(W)).toEqual([]);
  });

  it("D16: a stuck funding refuses the identity's top-ups, before the grant, until Repair resolves its step", async () => {
    install("dashpay-top-up-stuck");
    const step = topUpStep(0, 1);
    expect(await cmd.creditsStuckTopUpFundings(W)).toEqual([{ step, identityIndex: 0, identity: A, state: "maybeSent" }]);
    // The quote does not look at the funding steps (m4 §2.8); `top_up` refuses and keeps the grant.
    expect((await cmd.creditsTopUpQuote(W, A, 5_000_000)).totalDuffs).toBe(5_002_260);
    const g = await grant("platformOp", 5_002_260);
    expect(await code(cmd.creditsTopUp(W, A, 5_000_000, g))).toBe("platform.broadcast_unknown");
    await cmd.resolveStuckFunding(W, step, await grant("spend", 10_000_000));
    expect(await cmd.creditsStuckTopUpFundings(W)).toEqual([]);
    expect((await cmd.creditsTopUp(W, A, 5_000_000, g)).state.kind).toBe("applied");
  });

  it("T8: a funding left unknown refuses with its step and lists it stuck", async () => {
    install("dashpay-top-up-lost");
    const e = await cmd.creditsTopUp(W, A, 5_000_000, await grant("platformOp", 5_002_260)).catch((x: { code: string; data: unknown }) => x);
    expect(e).toMatchObject({ code: "platform.broadcast_unknown", data: { kind: "artifact", artifact: topUpStep(0, 1) } });
    expect(await cmd.creditsStuckTopUpFundings(W)).toEqual([{ step: topUpStep(0, 1), identityIndex: 0, identity: A, state: "maybeSent" }]);
  });
});

describe("contest_status (DP1-04's call, DP1-10 R1–R5)", () => {
  it("reads the status's own contest, or the one a scenario set", async () => {
    install("dashpay-contest-pending");
    expect(await cmd.contestStatus(W, A, "alice")).toMatchObject({ label: "alice", state: { kind: "open" }, endsAt: 1_900_000_000 });
    expect(await code(cmd.contestStatus(W, A, "bob"))).toBe("invalid_argument");
    install("dashpay-contest-lost");
    expect((await cmd.contestStatus(W, A, "alice")).state).toEqual({ kind: "lost", winner: IDENTITY.mallory });
  });
});

describe("credits withdrawal (DP6-02)", () => {
  const TO = "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf";
  async function credits(maxCredits: string) {
    return (await cmd.authorize({ kind: "platformOp", maxDuffs: 0, maxCredits }, W, { kind: "passphrase", passphrase: "demo" })).id;
  }

  it("Max keeps the fee in reserve in whole duffs; withdraws under the quote's grant and lists the document", async () => {
    install("dashpay-registered");
    const balance = BigInt((await cmd.identities(W))[0].balance!);
    const quote = await cmd.creditsWithdrawQuote(W, A, { kind: "all" });
    expect(BigInt(quote.credits)).toBe(balance - WITHDRAW_RULES.fee - ((balance - WITHDRAW_RULES.fee) % 1000n));
    expect(quote.grant).toEqual({ maxDuffs: 0, maxCredits: String(BigInt(quote.credits) + WITHDRAW_RULES.fee) });
    expect(await code(cmd.creditsWithdraw(W, A, TO, { kind: "all" }, await credits("1")))).toBe("platform.grant_exceeded");
    const outcome = await cmd.creditsWithdraw(W, A, TO, { kind: "all" }, await credits(quote.grant.maxCredits));
    expect(outcome).toMatchObject({ credits: quote.credits, expectedDuffs: quote.expectedDuffs, withdrawal: withdrawalId(1) });
    expect(BigInt(outcome.remainingCredits!)).toBe(balance - BigInt(quote.credits) - WITHDRAW_RULES.fee);
    expect((await cmd.identities(W))[0].balance).toBe(outcome.remainingCredits);
    expect(await cmd.creditsWithdrawals(W, A)).toMatchObject([{ id: withdrawalId(1), status: "queued", to: TO, credits: quote.credits }]);
  });

  it("refuses outside the range, past the balance, and to an address that is not Dash's", async () => {
    install("dashpay-registered");
    expect(await code(cmd.creditsWithdrawQuote(W, A, { kind: "credits", credits: "1000" }))).toBe("credits.withdrawal_out_of_range");
    expect(await code(cmd.creditsWithdrawQuote(W, A, { kind: "credits", credits: "50000000000001" }))).toBe("credits.withdrawal_out_of_range");
    expect(await code(cmd.creditsWithdrawQuote(W, A, { kind: "credits", credits: "184400000000" }))).toBe("platform.insufficient_credits");
    expect(await code(cmd.creditsWithdraw(W, A, "Xmainnet", { kind: "all" }, await credits("1")))).toBe("invalid_argument");
    install("dashpay-low-credits");
    expect(await code(cmd.creditsWithdrawQuote(W, A, { kind: "all" }))).toBe("platform.insufficient_credits");
  });

  it("an unanswered broadcast: unknown when its document is not found, landed when it is", async () => {
    install("dashpay-withdraw-lost");
    const q = await cmd.creditsWithdrawQuote(W, A, { kind: "all" });
    expect(await code(cmd.creditsWithdraw(W, A, TO, { kind: "all" }, await credits(q.grant.maxCredits)))).toBe("platform.broadcast_unknown");
    install("dashpay-withdraw-landed");
    const landed = await cmd.creditsWithdraw(W, A, TO, { kind: "all" }, await credits(q.grant.maxCredits));
    expect(landed.remainingCredits).toBeNull();
  });

  it("the withdrawals scenario lists one document per status, newest first", async () => {
    install("dashpay-withdrawals");
    const list = await cmd.creditsWithdrawals(W, A);
    expect(list.map((w) => w.status)).toEqual(["queued", "expired", "complete", "failed"]);
    expect(list.map((w) => w.updatedAt)).toEqual([...list.map((w) => w.updatedAt)].sort((a, b) => b! - a!));
  });
});
