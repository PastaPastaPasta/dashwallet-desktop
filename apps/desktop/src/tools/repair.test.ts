// Tools ▸ Repair, by parity ID (the Swift `ToolsViewModelTests`, ported), and the stuck funding
// Repair (E0-04 C9).

import { describe, expect, it, vi } from "vitest";
import type { FeePolicy } from "../api/generated/FeePolicy";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import type { RescanStart } from "../api/generated/RescanStart";
import type { RescanStatus } from "../api/generated/RescanStatus";
import { appError, WALLET_A } from "../onboarding/testWorld";
import { fakeVault } from "./testApi";
import { GrantModel } from "./grant";
import { RepairModel, RepairText, stuckFundingText } from "./repair";

const reg = (draft: string, waiting: RegistrationStatus["waiting"]): RegistrationStatus => ({
  draft,
  phase: "fundingSent",
  label: "alice",
  temporaryLabel: null,
  identity: null,
  txid: null,
  waiting,
  holdsKey: false,
  fundsCommitted: true,
  contestEndsAt: null,
  failure: null,
  nameFailure: null,
  profileFailure: null,
  createdAt: 0,
  updatedAt: 0,
});

function setup(lock = {}) {
  const calls: string[] = [];
  const vault = fakeVault(lock);
  let progress: RescanStatus | null = null;
  const api = {
    rescan: vi.fn(async (from: RescanStart) => {
      calls.push(`rescan ${from.kind}`);
      progress = { fromHeight: 0, currentHeight: 250, targetHeight: 1000, startedAt: 0 };
      return null;
    }),
    rescanProgress: vi.fn(async () => progress),
    cancelRescan: vi.fn(async () => {
      progress = null;
      return true;
    }),
    stopSpv: vi.fn(async () => (calls.push("stop"), null)),
    startSpv: vi.fn(async () => (calls.push("start"), null)),
    resetChainData: vi.fn(async () => (calls.push("reset"), null)),
    setBirthHeight: vi.fn(async (_: string, height: number) => {
      if (height > 1_200_000) throw appError("height_out_of_range");
      calls.push(`birth ${height}`);
      return null;
    }),
    dropUnconfirmed: vi.fn(async () => 2),
    registrations: vi.fn(async () => [reg("r1", "fundingUnknown"), reg("r2", "instantSend")]),
    resolveStuckFunding: vi.fn(async (walletId: string, step: string, grant: string) => {
      calls.push(`resolve ${walletId.slice(0, 2)} ${step} ${grant}`);
      return { txid: "ab".repeat(32) };
    }),
    feePolicy: vi.fn(async () => ({ maxTxFee: 10_000_000 }) as FeePolicy),
  };
  const grants = new GrantModel(vault.api);
  return { model: new RepairModel(api, grants, WALLET_A), api, calls, vault, grants };
}

describe("Repair (QT-117, QT-148, IOS-034, IOS-113)", () => {
  it("QT117_rescanWithProgressAndCancel", async () => {
    const { model, calls } = setup();
    await model.rescan({ kind: "genesis" });
    expect(calls).toEqual(["rescan genesis"]);
    expect(model.progressText).toBe("Rescanning… 250 / 1000");
    expect(model.progressFraction).toBe(0.25);
    await model.cancelRescan();
    expect(model.rescanning).toBe(false);
  });

  it("QT117_rescanWhileRescanningSaysWalletIsRescanning", async () => {
    const { model, api } = setup();
    api.rescan.mockRejectedValueOnce(appError("rescan_in_progress"));
    await model.rescan({ kind: "walletBirth" });
    expect(model.state).toEqual({ kind: "failed", text: "Rescan unavailable: Wallet is currently rescanning. Abort existing rescan or wait." });
  });

  it("QT148_resetChainDataAsksFirst", async () => {
    const { model, calls } = setup();
    model.ask("resetChainData");
    expect(model.state).toEqual({ kind: "confirming", what: "resetChainData" });
    model.dismiss();
    expect(calls).toEqual([]);
    model.ask("resetChainData");
    await model.confirm();
    expect(calls).toEqual(["stop", "reset", "start"]);
    expect(model.state).toEqual({ kind: "done", text: RepairText.resetDone });
  });

  it("QT148_resetWhileSPVRunsReportsTheEngineCode, and sync starts again", async () => {
    const { model, api, calls } = setup();
    api.resetChainData.mockRejectedValueOnce(appError("spv_running"));
    model.ask("resetChainData");
    await model.confirm();
    expect(model.state).toEqual({ kind: "failed", text: "Stop syncing first." });
    expect(calls).toEqual(["stop", "start"]);
  });

  it("QT117_rescanFromATypedHeightNeedsAWholeNumber", async () => {
    const { model, api } = setup();
    await model.rescanFrom("12.5");
    expect(model.state).toEqual({ kind: "failed", text: RepairText.birthHeightInvalid });
    expect(api.rescan).not.toHaveBeenCalled();
    await model.rescanFrom(" 900000 ");
    expect(api.rescan).toHaveBeenCalledExactlyOnceWith({ kind: "height", height: 900_000 });
  });

  it("IOS034_dropUnconfirmedCountsAndSchedulesRescan", async () => {
    const { model, api } = setup();
    model.ask("dropUnconfirmed");
    await model.confirm();
    expect(model.state).toEqual({ kind: "done", text: "2 transactions were removed; a rescan was scheduled." });
    expect(api.dropUnconfirmed).toHaveBeenCalledExactlyOnceWith(WALLET_A);
    expect(RepairText.dropped(1)).toBe("1 transaction was removed; a rescan was scheduled.");
  });

  it("IOS113_birthHeightAboveTipIsRefused", async () => {
    const { model, calls } = setup();
    await model.setBirthHeight("abc");
    expect(model.state).toEqual({ kind: "failed", text: RepairText.birthHeightInvalid });
    await model.setBirthHeight("-1");
    expect(model.state).toEqual({ kind: "failed", text: RepairText.birthHeightInvalid });
    await model.setBirthHeight("2000000");
    expect(model.state).toEqual({ kind: "failed", text: "The height is above the current chain tip." });
    await model.setBirthHeight(" 900000 ");
    expect(model.state).toEqual({ kind: "done", text: RepairText.birthHeightSaved(900_000) });
    expect(calls).toEqual(["birth 900000"]);
  });
});

describe("Stuck funding (E0-04 C9, U-05 D12)", () => {
  it("lists only registrations whose funding is unknown", async () => {
    const { model } = setup();
    await model.refresh();
    expect(model.stuck.map((r) => r.draft)).toEqual(["r1"]);
  });

  it("sends all coins to yourself under a fee-capped spend grant", async () => {
    const { model, calls, vault } = setup({ encrypted: false });
    await model.refresh();
    await model.resolveStuck(model.stuck[0]);
    expect(vault.calls.authorize[0]).toMatchObject({ purpose: { kind: "spend", maxDuffs: 10_000_000 }, walletId: WALLET_A });
    expect(calls).toEqual([`resolve a1 registration/r1/funding grant-1`]);
    expect(model.state).toMatchObject({ kind: "done" });
  });

  it("a refusal names why, and the unspent grant is revoked", async () => {
    const { model, api, vault } = setup({ encrypted: false });
    api.resolveStuckFunding.mockRejectedValueOnce(appError("stuck_funding", { kind: "stuckFunding", refusal: { kind: "callRunning" }, retryLater: true }));
    await model.resolveStuck(reg("r1", "fundingUnknown"));
    expect(model.state).toEqual({ kind: "failed", text: "The funding is still being sent. Try again later." });
    expect(vault.calls.revoked).toEqual(["grant-1"]);
    expect(stuckFundingText({ kind: "lockInputsSpent", txid: "cd" }, false)).toBe("The funding's coins were already spent by transaction cd.");
  });

  it("a cancelled passphrase sends nothing", async () => {
    const { model, grants, calls } = setup({ encrypted: true });
    const running = model.resolveStuck(reg("r1", "fundingUnknown"));
    await vi.waitFor(() => expect(grants.pending).not.toBeNull());
    expect(model.busy).toBe(true);
    grants.cancel();
    await running;
    expect(calls).toEqual([]);
    expect(model.state).toEqual({ kind: "idle" });
  });
});
