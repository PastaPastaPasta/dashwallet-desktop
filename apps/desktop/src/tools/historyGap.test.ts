import { describe, expect, it } from "vitest";
import type { WalletSummary } from "../api/generated/WalletSummary";
import { appError } from "../onboarding/testWorld";
import { historyGapModel, HistoryGapModel, HistoryGapText } from "./historyGap";
import { RepairText } from "./repair";

const wallet = (walletId: string, name: string): WalletSummary => ({ walletId, name, watchOnly: false, hasMnemonic: true, hd: true, birthHeight: 900_000, createdAt: null, balances: null });

function fakeApi(fail?: "stop" | "reset") {
  const calls: string[] = [];
  const step = (name: string) => async () => {
    calls.push(name);
    if (fail === name) throw appError(name === "reset" ? "spv_running" : "internal");
    return null;
  };
  return { calls, api: { stopSpv: step("stop"), resetChainData: step("reset"), startSpv: step("start") } };
}

describe("history gap notice", () => {
  it("names the one wallet and counts several", () => {
    const wallets = [wallet("a", "Main"), wallet("b", "Savings")];
    expect(HistoryGapText.body({ startHeight: 1_100_000, walletIds: ["a"] }, wallets)).toBe(
      "The history of “Main” starts before block 1100000, where this app's block data begins. Its payments before that block aren't shown yet, so its balance may be missing coins.",
    );
    expect(HistoryGapText.body({ startHeight: 1_100_000, walletIds: ["a", "b"] }, wallets)).toBe(
      "The history of 2 wallets starts before block 1100000, where this app's block data begins. Their payments before that block aren't shown yet, so their balances may be missing coins.",
    );
  });

  it("offers the reset under Repair's own label", () => {
    expect(HistoryGapText.reset).toBe("Reset chain data and resync…");
    expect(HistoryGapText.reset).toBe(RepairText.resetChainDataButton);
  });

  it("resets with sync stopped, then starts sync again", async () => {
    const { calls, api } = fakeApi();
    const model = new HistoryGapModel(api);
    expect(await model.confirm()).toBe(false);
    model.ask();
    expect(model.state.kind).toBe("confirming");
    expect(await model.confirm()).toBe(true);
    expect(calls).toEqual(["stop", "reset", "start"]);
    expect(model.state.kind).toBe("shown");
  });

  it("a failed reset says why and starts sync again", async () => {
    const { calls, api } = fakeApi("reset");
    const model = new HistoryGapModel(api);
    model.ask();
    expect(await model.confirm()).toBe(false);
    expect(calls).toEqual(["stop", "reset", "start"]);
    expect(model.state).toEqual({ kind: "failed", text: "Stop syncing first." });
  });

  it("Not now and a reset under way outlive Home's mounts", async () => {
    const { api } = fakeApi();
    const model = historyGapModel(api);
    expect(historyGapModel(api)).toBe(model);
    model.ask();
    model.dismiss();
    expect(model.state.kind).toBe("shown");
    model.notNow();
    expect(historyGapModel(api).state.kind).toBe("hidden");
    expect(historyGapModel(fakeApi().api).state.kind).toBe("shown");

    const other = fakeApi().api;
    historyGapModel(other).ask();
    const done = historyGapModel(other).confirm();
    expect(historyGapModel(other).state.kind).toBe("working");
    historyGapModel(other).notNow();
    expect(historyGapModel(other).state.kind).toBe("working");
    expect(await done).toBe(true);
  });
});
