// The network switch's page side (row 74): the listener entries Tauri keeps across the reboots,
// what each switch failure says, and the note the next page shows.

import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IpcBackend, liveListenerCount } from "../api/tauri";
import { bootAgain, switchOutcome, takeSwitchNote } from "./switch";

let stored: Map<string, string>;
beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  stored = new Map();
  vi.stubGlobal("sessionStorage", { getItem: (k: string) => stored.get(k) ?? null, setItem: (k: string, v: string) => stored.set(k, v), removeItem: (k: string) => stored.delete(k) });
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

describe("a switch's reboot", () => {
  it("leaves Tauri one engine-event and one shell-event entry per page, however many switches", async () => {
    // dw-app's listener registry: it outlives the page, as Tauri's does across a reload.
    const registry = new Map<number, string>();
    let next = 0;
    mockIPC((cmd, args) => {
      const a = args as { event: string; eventId?: number };
      if (cmd === "plugin:event|listen") return registry.set(++next, a.event), next;
      if (cmd === "plugin:event|unlisten") return void registry.delete(a.eventId!);
      throw new Error(`unexpected ${cmd}`);
    });
    const reloads = vi.fn();
    for (let page = 0; page < 5; page++) {
      // What main.tsx subscribes on each boot.
      const backend = new IpcBackend("tauri");
      await backend.onEvent(() => undefined);
      await backend.onShellEvent(() => undefined);
      expect([...registry.values()].sort()).toEqual(["engine://event", "shell://event"].sort());
      expect(liveListenerCount()).toBe(2);
      await bootAgain(null, reloads);
      expect(registry.size).toBe(0);
      expect(liveListenerCount()).toBe(0);
    }
    expect(reloads).toHaveBeenCalledTimes(5);
    expect(next).toBe(10);
  });

  it("leaves the note for the next page, once", async () => {
    await bootAgain("Mainnet did not open. Dash Wallet is back on Testnet.", () => undefined);
    expect(takeSwitchNote()).toBe("Mainnet did not open. Dash Wallet is back on Testnet.");
    expect(takeSwitchNote()).toBeNull();
  });
});

describe("switchOutcome", () => {
  const busy = (flows: string[], mixing: boolean, broadcast: boolean) => ({
    code: "app.network_busy",
    message: "busy",
    data: { kind: "networkBusy", flows, mixing, broadcast },
  });

  it("says what a busy network waits for", () => {
    expect(switchOutcome(busy(["registration"], false, false), "mainnet", "testnet")).toEqual({
      problem: "Wait for username registration to finish before switching networks.",
    });
    expect(switchOutcome(busy([], false, true), "mainnet", "testnet")).toEqual({ problem: "Wait for the payment to finish before switching networks." });
    expect(switchOutcome(busy([], true, false), "mainnet", "testnet")).toEqual({ problem: "Stop mixing before switching networks." });
    expect(switchOutcome(busy(["topUp", "withdraw"], true, true), "mainnet", "testnet")).toEqual({
      problem: "Wait for the payment, credit top-up and credit withdrawal to finish, or stop mixing, before switching networks.",
    });
  });

  it("gives close's own refusals their own words, and any other failure the generic one", () => {
    expect(switchOutcome({ code: "wallet", message: "opening", data: null }, "mainnet", "testnet")).toEqual({
      problem: "A wallet is still opening. Switch networks once it is open.",
    });
    expect(switchOutcome({ code: "shielded_spend_unresolved", message: "x", data: null }, "testnet", "mainnet")).toEqual({
      problem: "A shielded payment is still being confirmed. Switch networks once it is.",
    });
    expect(switchOutcome({ code: "storage_in_use", message: "the data directory is in use", data: null }, "testnet", "mainnet")).toEqual({
      problem: "Dash Wallet could not switch networks. the data directory is in use",
    });
  });

  it("boots again after a failure that closed the network", () => {
    const failed = (rolledBack: boolean) => ({
      code: "app.network_switch_failed",
      message: "x",
      data: { kind: "networkSwitch", network: { kind: "mainnet", dirName: "mainnet", unitName: "DASH" }, rolledBack },
    });
    expect(switchOutcome(failed(true), "mainnet", "testnet")).toEqual({ reboot: "Mainnet did not open. Dash Wallet is back on Testnet." });
    expect(switchOutcome(failed(false), "mainnet", "testnet")).toEqual({ reboot: "Mainnet did not open, and neither did Testnet again." });
  });
});
