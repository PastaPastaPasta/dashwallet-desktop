// The CoinJoin view model (U-06): one test per S-row of docs/design/U-06-coinjoin.md, the level
// mapping (D2), Settings ▸ CoinJoin's checks (D11) and the start/stop failure texts (D9).

import { describe, expect, it } from "vitest";
import type { AppError } from "../api/generated/commands";
import type { CoinJoinLimits } from "../api/generated/CoinJoinLimits";
import type { CoinJoinStatus } from "../api/generated/CoinJoinStatus";
import {
  actionFailureText,
  announcement,
  checkSettings,
  CoinJoinText,
  coinjoinView,
  formOf,
  levelOf,
  lockLine,
  settingsFailureText,
  statusLine,
  type ViewInputs,
} from "./model";

const amount = (duffs: number) => `${(duffs / 1e8).toFixed(8)} tDASH`;

function status(patch: Partial<CoinJoinStatus> = {}): CoinJoinStatus {
  return {
    walletId: "w",
    state: "idle",
    stopReason: null,
    unavailable: null,
    balances: { anonymizable: 150_000_000, denominated: 50_000_000, normalizedAnonymized: 25_000_000, fullyMixed: 25_000_000 },
    progress: { overallPercent: 42.7, denominatedPercent: 100, partiallyMixedPercent: 60, mixedPercent: 30, averageRounds: 1.5 },
    amountAndRounds: { amount: 100_000_000, rounds: 4, insufficientInputs: false },
    submittedDenominations: [],
    sessions: [],
    status: { kind: "idle" },
    queueSize: 0,
    keysLeft: null,
    ...patch,
  };
}

const view = (s: CoinJoinStatus | undefined, extra: Partial<ViewInputs> = {}) => coinjoinView({ status: s, readFailed: false, amount, masked: null, ...extra });

const LIMITS: CoinJoinLimits = {
  denominations: [],
  minMixingBalance: 140_001,
  collateralAmount: 10_000,
  maxCollateralAmount: 40_000,
  minRounds: 2,
  maxRounds: 16,
  minSessions: 1,
  maxSessions: 10,
  minAmountDash: 2,
  maxAmountDash: 21_000_000,
  minDenoms: 10,
  maxDenoms: 100_000,
  defaults: { enabled: true, multiSession: false, maxSessions: 4, rounds: 4, targetAmountDash: 1000, denomsGoal: 50, denomsHardCap: 300 },
};

const appError = (code: AppError["code"], data: AppError["data"] = null): AppError => ({ code, message: "m", data });

describe("coinjoinView", () => {
  it("S1 loading: no status yet, nothing on Home", () => {
    expect(view(undefined)).toMatchObject({ state: "loading", action: null, onHome: false, percent: null });
  });

  it("S2 readFailed: Try again, also over a cached status", () => {
    const v = view(status({ state: "mixing" }), { readFailed: true, readError: new Error("boom") });
    expect(v).toMatchObject({ state: "readFailed", title: CoinJoinText.readFailed, action: { kind: "retry" }, onHome: false });
  });

  it("S3 stopping: a disabled Stop and a spinner", () => {
    expect(view(status({ state: "stopping" }))).toMatchObject({ state: "stopping", action: { kind: "stop", disabled: true }, busy: true, onHome: true, line: null });
  });

  it("S4 starting: mixing whose first step has not run", () => {
    const atStart = { balances: status().balances, total: 300_000_000 };
    expect(view(status({ state: "mixing" }), { atStart, total: 300_000_000 })).toMatchObject({
      state: "starting",
      title: CoinJoinText.starting,
      action: { kind: "stop", disabled: false },
      busy: true,
      onHome: true,
    });
  });

  it("S4 ends with the first step: idle after a funding step is Mixing (D16, review Sol r1 #1)", () => {
    const b = status().balances;
    const atStart = { balances: b, total: 300_000_000 };
    // The step made denominations and reported nothing; the engine then waits a block, still idle.
    const funded = status({ state: "mixing", balances: { ...b, anonymizable: b.anonymizable - 1_000, denominated: b.denominated + 40_000_000 } });
    expect(view(funded, { atStart, total: 299_999_000 })).toMatchObject({ state: "mixing", title: CoinJoinText.mixing, line: null, busy: false, action: { kind: "stop" } });
    // A collateral step from change below the smallest denomination moves none of the four
    // balances: its fee moves the wallet's total.
    expect(view(status({ state: "mixing" }), { atStart, total: 299_999_000 }).state).toBe("mixing");
    // A window that never read the mixer stopped cannot tell: the mixer runs.
    expect(view(status({ state: "mixing" }))).toMatchObject({ state: "mixing", line: null });
  });

  it("S5 paused: a waiting code shows Paused with Core's line (D4)", () => {
    for (const kind of ["syncInProgress", "noMasternodes", "notEnoughFunds", "unconfirmedDenominated", "walletLocked"] as const) {
      const v = view(status({ state: "mixing", status: { kind } }));
      expect(v, kind).toMatchObject({ state: "paused", title: CoinJoinText.paused, line: statusLine({ kind }), onHome: true, busy: false });
    }
    expect(view(status({ state: "mixing", status: { kind: "noMasternodes" } })).line).toBe("No Masternodes detected.");
  });

  it("S6 finished: running at 100 %, also while it waits for new coins", () => {
    const full = { ...status().progress, overallPercent: 100 };
    expect(view(status({ state: "mixing", status: { kind: "mixingInProgress" }, progress: full }))).toMatchObject({
      state: "finished",
      title: CoinJoinText.fullyMixed,
      percent: 100,
      action: { kind: "stop" },
    });
    expect(view(status({ state: "mixing", status: { kind: "notEnoughFunds" }, progress: full })).state).toBe("finished");
    // Above the target the engine's step does nothing and reports idle: not "Starting…" (review r1 #1).
    expect(view(status({ state: "mixing", status: { kind: "idle" }, progress: full }))).toMatchObject({ state: "finished", busy: false, line: null });
  });

  it("S7 mixing: percent floored, mixed of total, Core's line", () => {
    const v = view(status({ state: "mixing", status: { kind: "waitingInQueue" } }));
    expect(v).toMatchObject({ state: "mixing", title: CoinJoinText.mixing, percent: 42, line: "Submitted to masternode, waiting in queue ." });
    expect(v.mixed).toBe("0.25000000 tDASH of 1.00000000 tDASH");
    expect(view(status({ state: "mixing", status: { kind: "masternode", message: "queueFull" } })).line).toBe("Masternode: Masternode queue is full.");
  });

  it("D3: a target lowered below the mixed balance shows the target as mixed (review Sol r1 #2)", () => {
    // 10 DASH mixed, the target lowered to 2: the engine's amount is the target, its percent 100.
    const lowered = status({
      state: "mixing",
      balances: { ...status().balances, fullyMixed: 1_000_000_000 },
      amountAndRounds: { amount: 200_000_000, rounds: 4, insufficientInputs: false },
      progress: { ...status().progress, overallPercent: 100 },
    });
    expect(view(lowered)).toMatchObject({ state: "finished", mixed: "2.00000000 tDASH of 2.00000000 tDASH" });
    expect(view({ ...lowered, state: "idle" }).mixed).toBe("2.00000000 tDASH of 2.00000000 tDASH");
  });

  it("masks the amounts in discreet mode", () => {
    expect(view(status(), { masked: "*** tDASH" }).mixed).toBe("*** tDASH of *** tDASH");
  });

  it("S8 disabled: Open Settings, not on Home", () => {
    expect(view(status({ unavailable: { kind: "disabled" } }))).toMatchObject({ state: "disabled", action: { kind: "settings" }, onHome: false });
  });

  it("S9 watchOnly: no button", () => {
    expect(view(status({ unavailable: { kind: "watchOnly" } }))).toMatchObject({ state: "watchOnly", title: CoinJoinText.watchOnly, action: null, onHome: false });
  });

  it("S10 locked and S11 error: on Home with Dismiss and Start (D1)", () => {
    expect(view(status({ stopReason: "vaultLocked" }))).toMatchObject({
      state: "locked",
      line: CoinJoinText.stoppedLocked,
      action: { kind: "start", disabled: false },
      onHome: true,
      dismissable: true,
    });
    expect(view(status({ stopReason: "sessionClosed" }))).toMatchObject({ state: "error", line: CoinJoinText.stoppedSession, dismissable: true });
    expect(view(status({ stopReason: "walletUnloaded" }))).toMatchObject({ state: "error", line: CoinJoinText.stoppedWallet });
    // Start is offered only when the engine would take it.
    expect(view(status({ stopReason: "vaultLocked", unavailable: { kind: "insufficientFunds", minDuffs: 140_001 } })).action).toMatchObject({ disabled: true });
  });

  it("S12 tooLittle: the minimum and a disabled Start", () => {
    const v = view(status({ unavailable: { kind: "insufficientFunds", minDuffs: 140_001 } }));
    expect(v).toMatchObject({ state: "tooLittle", line: "CoinJoin requires at least 0.00140001 tDASH to use.", action: { kind: "start", disabled: true }, onHome: false });
  });

  it("S13 done and S14 off; a stop asked for or by Settings is not news", () => {
    expect(view(status({ progress: { ...status().progress, overallPercent: 100 } }))).toMatchObject({ state: "done", title: CoinJoinText.fullyMixed, onHome: false });
    for (const stopReason of [null, "userRequested", "disabled"] as const)
      expect(view(status({ stopReason })), String(stopReason)).toMatchObject({ state: "off", title: CoinJoinText.notStarted, action: { kind: "start" }, onHome: false });
  });
});

describe("levels (D2)", () => {
  it("maps rounds to Intermediate, Advanced or Custom", () => {
    expect(levelOf(4)).toBe("intermediate");
    expect(levelOf(8)).toBe("advanced");
    expect(levelOf(6)).toBe("custom");
    expect(CoinJoinText.custom(6)).toBe("Custom: 6 rounds");
  });
});

describe("checkSettings (D11)", () => {
  const current = LIMITS.defaults;
  it("takes the form within the engine's ranges", () => {
    expect(checkSettings({ ...formOf(current), rounds: "8", targetAmountDash: "2,500" }, current, LIMITS)).toEqual({ ok: { ...current, rounds: 8, targetAmountDash: 2500 } });
  });

  it("names the first field out of range", () => {
    expect(checkSettings({ ...formOf(current), rounds: "1" }, current, LIMITS)).toEqual({ field: "rounds", text: "Mixing rounds must be a whole number from 2 to 16." });
    expect(checkSettings({ ...formOf(current), targetAmountDash: "1.5" }, current, LIMITS)).toMatchObject({ field: "targetAmountDash" });
    expect(checkSettings({ ...formOf(current), maxSessions: "" }, current, LIMITS)).toMatchObject({ field: "maxSessions" });
    // A comma only as a thousands separator.
    expect(checkSettings({ ...formOf(current), targetAmountDash: "2,5,00" }, current, LIMITS)).toMatchObject({ field: "targetAmountDash" });
    expect(checkSettings({ ...formOf(current), denomsHardCap: "100001" }, current, LIMITS)).toEqual({
      field: "denomsHardCap",
      text: "Inputs per denomination: Maximum must be a whole number from 10 to 100,000.",
    });
  });

  it("refuses a target above the maximum", () => {
    expect(checkSettings({ ...formOf(current), denomsGoal: "400" }, current, LIMITS)).toEqual({ field: "denomsGoal", text: "The target number of inputs cannot be above the maximum." });
  });
});

describe("texts", () => {
  it("D9: a failed start says why", () => {
    expect(actionFailureText(appError("coinjoin.vault_locked"), amount)).toBe(CoinJoinText.vaultLocked);
    expect(actionFailureText(appError("coinjoin.disabled"), amount)).toBe(CoinJoinText.disabled);
    expect(actionFailureText(appError("coinjoin.watch_only"), amount)).toBe(CoinJoinText.watchOnly);
    expect(actionFailureText(appError("coinjoin.insufficient_funds", { kind: "limit", duffs: 140_001 }), amount)).toBe(
      "CoinJoin requires at least 0.00140001 tDASH to use.",
    );
  });

  it("D11: the engine's refusal of a setting names it", () => {
    expect(settingsFailureText({ code: "invalid_argument", message: "rounds = 1 is outside 2..=16", data: null })).toBe(
      "The settings were not saved: rounds = 1 is outside 2..=16",
    );
  });

  it("announces the state, and a stop's reason, not the moving status line", () => {
    expect(announcement(view(status({ state: "mixing", status: { kind: "signing" } })))).toBe(CoinJoinText.mixing);
    expect(announcement(view(status({ stopReason: "vaultLocked" })))).toBe(`${CoinJoinText.stopped}. ${CoinJoinText.stoppedLocked}`);
    expect(announcement(view(status({ unavailable: { kind: "insufficientFunds", minDuffs: 140_001 } })))).toContain("requires at least");
  });

  it("the lock line follows the vault", () => {
    expect(lockLine("unlockedMixingOnly")).toBe(CoinJoinText.mixingOnly);
    expect(lockLine("unlocked")).toBe(CoinJoinText.mixingOnlyHow);
    expect(lockLine("unencrypted")).toBeNull();
  });
});
