// The fixture's CoinJoin (coinjoin.ts, U-06) through the generated commands, as the CoinJoin
// surfaces call them: the engine's start, stop and halt rules, its codes and its signal.

import { clearMocks } from "@tauri-apps/api/mocks";
import { listen } from "@tauri-apps/api/event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import type { AppEvent } from "../api/generated/commands";
import { COINJOIN_LIMITS } from "./coinjoin";
import { installFixture, type FixtureHandle } from "./index";
import { restarted } from "./registration";
import { FIXTURE_PASSPHRASE, FIXTURE_WALLET_ID as W, type ScenarioName } from "./scenarios";

let fixture: FixtureHandle;
const install = (scenario: ScenarioName) => (fixture = installFixture({ scenario }));
beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  install("funded");
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

async function events() {
  const seen: AppEvent[] = [];
  await listen<AppEvent>(cmd.ENGINE_EVENT, (e) => void seen.push(e.payload));
  return seen;
}

const coinJoinEvents = (seen: AppEvent[]) => seen.filter((e) => e.kind === "coinJoin").map((e) => (e.kind === "coinJoin" ? e.walletId : null));

describe("fixture CoinJoin", () => {
  it("answers dw-coinjoin's limits and defaults", async () => {
    expect(await cmd.coinjoinLimits()).toEqual(COINJOIN_LIMITS);
    expect(await cmd.coinjoinSettings()).toEqual(COINJOIN_LIMITS.defaults);
  });

  it("starts and stops a wallet's mixer, with the engine's signal", async () => {
    const seen = await events();
    expect(await cmd.coinjoinStatus(W)).toMatchObject({ state: "idle", stopReason: null, unavailable: null, status: { kind: "idle" } });
    await cmd.startMixing(W);
    expect(await cmd.coinjoinStatus(W)).toMatchObject({ state: "mixing", stopReason: null });
    // A second start is a no-op, as the engine's.
    await cmd.startMixing(W);
    await cmd.stopMixing(W);
    expect(await cmd.coinjoinStatus(W)).toMatchObject({ state: "idle", stopReason: "userRequested" });
    // Stopping a stopped wallet changes nothing and says nothing.
    await cmd.stopMixing(W);
    expect(coinJoinEvents(seen)).toEqual([W, W]);
  });

  it("refuses to start with the engine's codes", async () => {
    install("empty");
    await expect(cmd.startMixing(W)).rejects.toMatchObject({
      code: "coinjoin.insufficient_funds",
      data: { kind: "limit", duffs: COINJOIN_LIMITS.minMixingBalance },
    });
    expect((await cmd.coinjoinStatus(W)).unavailable).toEqual({ kind: "insufficientFunds", minDuffs: COINJOIN_LIMITS.minMixingBalance });
    install("coinjoin-watch-only");
    await expect(cmd.startMixing(W)).rejects.toMatchObject({ code: "coinjoin.watch_only" });
    install("coinjoin-disabled");
    await expect(cmd.startMixing(W)).rejects.toMatchObject({ code: "coinjoin.disabled" });
    expect((await cmd.coinjoinStatus(W)).unavailable).toEqual({ kind: "disabled" });
  });

  it("stops mixing when the vault locks, and starts unlocked for mixing only", async () => {
    install("coinjoin-mixing");
    await cmd.lock();
    expect(await cmd.coinjoinStatus(W)).toMatchObject({ state: "idle", stopReason: "vaultLocked", sessions: [] });
    await expect(cmd.startMixing(W)).rejects.toMatchObject({ code: "coinjoin.vault_locked" });
    await cmd.unlockMixingOnly(FIXTURE_PASSPHRASE);
    await cmd.startMixing(W);
    expect(await cmd.coinjoinStatus(W)).toMatchObject({ state: "mixing", stopReason: null });
  });

  it("checks the settings' ranges, and turning CoinJoin off stops every wallet", async () => {
    install("coinjoin-mixing");
    const current = await cmd.coinjoinSettings();
    for (const bad of [{ rounds: 1 }, { rounds: 17 }, { maxSessions: 0 }, { targetAmountDash: 1 }, { denomsGoal: 301 }, { denomsHardCap: 9 }])
      await expect(cmd.setCoinjoinSettings({ ...current, ...bad })).rejects.toMatchObject({ code: "invalid_argument" });
    expect(await cmd.coinjoinSettings()).toEqual(current);
    const seen = await events();
    await cmd.setCoinjoinSettings({ ...current, rounds: 8 });
    expect(await cmd.coinjoinStatus(W)).toMatchObject({ state: "mixing", amountAndRounds: { rounds: 8 } });
    await cmd.setCoinjoinSettings({ ...current, enabled: false });
    expect(await cmd.coinjoinStatus(W)).toMatchObject({ state: "idle", stopReason: "disabled", unavailable: { kind: "disabled" } });
    expect(coinJoinEvents(seen)).toContain(W);
  });

  it("forgets the mixers when dw-app restarts, as the engine keeps them in memory (D5)", () => {
    install("coinjoin-mixing");
    expect(restarted(structuredClone(fixture.world)).coinjoin.mixers).toEqual({});
  });

  it("derives mixed of total from the mixer's progress and the target", async () => {
    install("coinjoin-mixing");
    const s = await cmd.coinjoinStatus(W);
    const confirmed = fixture.world.wallets[0].summary.balances!.confirmed;
    expect(s.amountAndRounds.amount).toBe(Math.min(confirmed, COINJOIN_LIMITS.defaults.targetAmountDash * 100_000_000));
    expect(s.balances.fullyMixed).toBe(Math.floor((s.amountAndRounds.amount * s.progress.mixedPercent) / 100));
    expect(s.submittedDenominations).toEqual([100_001_000]);
  });
});
