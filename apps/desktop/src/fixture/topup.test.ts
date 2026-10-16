// The fixture's credits top-up and contest read (DP1-10) over the real IPC path: the quote's
// refusals in the engine's order, a top-up applied or left pending, Finish and "Credits arrived",
// a stuck funding that Repair resolves, and `contest_status`.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import { IDENTITY } from "./dashpay";
import { installFixture } from "./index";
import { FIXTURE_WALLET_ID, type ScenarioName } from "./scenarios";
import { TOP_UP_FEE_DUFFS, topUpStep } from "./topup";

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
