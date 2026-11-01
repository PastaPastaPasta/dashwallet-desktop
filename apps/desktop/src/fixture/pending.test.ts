// The fixture's answers for the commands dw-app does not have yet (pending.ts), through
// IpcBackend as the screens call them: the adapter's "absent" and "present" paths.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IpcBackend } from "../api/tauri";
import { installFixture, type FixtureHandle } from "./index";
import { FIXTURE_WALLET_ID as W } from "./scenarios";

let fixture: FixtureHandle;
let backend: IpcBackend;
beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  fixture = installFixture({ scenario: "funded" });
  backend = new IpcBackend("fixture");
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

describe("fixture pending commands", () => {
  it("reads and saves the network choice", async () => {
    expect(await backend.networkChoice()).toBe("testnet");
    expect(await backend.setNetworkChoice("mainnet")).toBeNull();
    expect(await backend.networkChoice()).toBe("mainnet");
    expect(fixture.calls).toContain("set_network_choice");
  });

  it("answers null for the choice, and refuses to save it, while the command is absent", async () => {
    fixture.world.pending.networkChoice = null;
    expect(await backend.networkChoice()).toBeNull();
    await expect(backend.setNetworkChoice("mainnet")).rejects.toThrow("not available");
  });

  it("reads the shielded and Platform balances, null while their commands are absent", async () => {
    expect(await backend.shieldedBalance(W)).toBe(125_000_000);
    expect(await backend.platformBalance(W)).toBe(40_000_000);
    fixture.world.pending.shielded = null;
    fixture.world.pending.platform = null;
    expect(await backend.shieldedBalance(W)).toBeNull();
    expect(await backend.platformBalance(W)).toBeNull();
  });

  it("exports the history as dash-qt's CSV: one quoted line per record, the header first", async () => {
    const all = await backend.historyPage(W, { sort: "newestFirst", cursor: null, limit: 500, categories: [], text: null });
    const csv = await backend.exportHistoryCsv(W, { categories: [], text: null }, "newestFirst", "dash", [], 0);
    const lines = csv.split("\n");
    expect(lines.at(-1)).toBe("");
    expect(lines[0]).toBe('"Confirmed","Date","Type","Label","Address","Amount (DASH)","ID"');
    expect(lines.length - 2).toBe(all.totalMatching);
    expect(lines[1]).toContain(`"${all.rows[0].txid}"`);
    const received = await backend.exportHistoryCsv(W, { categories: ["received"], text: null }, "newestFirst", "duffs", [], 0);
    expect(received.split("\n").length - 2).toBeLessThan(all.rows.length);
    expect(received).toContain("Amount (duffs)");
  });
});
