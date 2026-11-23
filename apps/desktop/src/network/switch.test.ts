// The network switch's page side (row 74): the listener entries Tauri keeps across the reboots,
// what each switch failure says, and the note the next page shows.

import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IpcBackend, liveListenerCount } from "../api/tauri";
import { bootAgain, switchOutcome, takeSwitchNote } from "./switch";

// The link takes asked for so far: a test sets one still in flight.
const takes = vi.hoisted(() => ({ pending: Promise.resolve() }));
vi.mock("../app/shellBridge", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../app/shellBridge")>()),
  linksTaken: () => takes.pending,
}));

let stored: Map<string, string>;
beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  stored = new Map();
  vi.stubGlobal("sessionStorage", { getItem: (k: string) => stored.get(k) ?? null, setItem: (k: string, v: string) => stored.set(k, v), removeItem: (k: string) => stored.delete(k) });
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
  takes.pending = Promise.resolve();
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

  it("waits for a link take still in flight, so its batch is kept, not lost with the page", async () => {
    let answer = () => {};
    takes.pending = new Promise<void>((resolve) => (answer = resolve));
    const reload = vi.fn();
    const booting = bootAgain(null, reload);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(reload).not.toHaveBeenCalled();
    answer();
    await booting;
    expect(reload).toHaveBeenCalledOnce();
  });

  it("leaves the note for the next page, once", async () => {
    await bootAgain("Mainnet did not open. Dash Wallet is back on Testnet.", () => undefined);
    expect(takeSwitchNote()).toBe("Mainnet did not open. Dash Wallet is back on Testnet.");
    expect(takeSwitchNote()).toBeNull();
  });
});

describe("switchOutcome", () => {
  const busy = (flows: string[], mixing: boolean, broadcast: boolean, more: { stalled?: boolean; draining?: boolean } = {}) => ({
    code: "app.network_busy",
    message: "busy",
    data: { kind: "networkBusy", flows, mixing, broadcast, stalled: false, draining: false, ...more },
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

  it("says a lock stops flows that wait for a proof or a grant, when the wallet can be locked", () => {
    const stalled = busy(["contactRequest"], false, false, { stalled: true });
    expect(switchOutcome(stalled, "mainnet", "testnet", true)).toEqual({
      problem: "Wait for contact request to finish before switching networks. Locking the wallet stops contact request.",
    });
    // An unencrypted vault has no lock.
    expect(switchOutcome(stalled, "mainnet", "testnet", false)).toEqual({
      problem: "Wait for contact request to finish before switching networks.",
    });
    // A flow that runs: only the wait.
    expect(switchOutcome(busy(["contactRequest"], false, false), "mainnet", "testnet", true)).toEqual({
      problem: "Wait for contact request to finish before switching networks.",
    });
  });

  it("says work that did not finish in time by its flag, and gives a busy reply naming nothing the generic words", () => {
    expect(switchOutcome(busy([], false, false, { draining: true }), "mainnet", "testnet")).toEqual({
      problem: "Dash Wallet is still finishing a request on this network. Try switching again once it is done.",
    });
    expect(switchOutcome(busy([], false, false), "mainnet", "testnet")).toEqual({
      problem: "Dash Wallet could not switch networks. busy (app.network_busy)",
    });
  });

  it("gives close's own refusals their own words by their data, and any other failure the generic one with its code", () => {
    const refused = (code: string, reason: string) => ({ code, message: "x", data: { kind: "networkCloseRefused", reason } });
    expect(switchOutcome(refused("wallet", "walletOpening"), "mainnet", "testnet")).toEqual({
      problem: "A wallet is still opening. Switch networks once it is open.",
    });
    expect(switchOutcome(refused("shielded_spend_unresolved", "shieldedSpendUnresolved"), "testnet", "mainnet")).toEqual({
      problem: "A shielded payment is still being confirmed. Switch networks once it is.",
    });
    // A reason this build does not know, and a `wallet` error that is not the close's refusal.
    expect(switchOutcome(refused("wallet", "later"), "mainnet", "testnet")).toEqual({ problem: "Dash Wallet could not switch networks. x (wallet)" });
    expect(switchOutcome({ code: "wallet", message: "the wallet file is damaged", data: null }, "mainnet", "testnet")).toEqual({
      problem: "Dash Wallet could not switch networks. the wallet file is damaged (wallet)",
    });
    expect(switchOutcome({ code: "storage_in_use", message: "the data directory is in use", data: null }, "testnet", "mainnet")).toEqual({
      problem: "Dash Wallet could not switch networks. the data directory is in use (storage_in_use)",
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
