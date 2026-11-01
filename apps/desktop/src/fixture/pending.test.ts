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
    const first = await backend.networkChoice();
    expect(first).toMatchObject({ chosen: "testnet", saved: true, forcedByFlag: false, restartNeeded: false });
    expect(await backend.setNetworkChoice("mainnet")).toMatchObject({ chosen: "mainnet", restartNeeded: true });
    expect(await backend.networkChoice()).toMatchObject({ chosen: "mainnet", restartNeeded: true });
    expect(await backend.setNetworkChoice("testnet")).toMatchObject({ restartNeeded: false });
    expect(fixture.calls).toContain("set_network_choice");
  });

  it("answers null for the choice, and refuses to save it, while the command is absent", async () => {
    fixture.world.pending.networkChoice = null;
    expect(await backend.networkChoice()).toBeNull();
    await expect(backend.setNetworkChoice("mainnet")).rejects.toThrow("not available");
  });

  it("reads the shielded and Platform balances, null while their commands are absent", async () => {
    expect(await backend.shieldedBalance(W)).toBe(125_000_000);
    const platform = await backend.platformAddresses(W);
    expect(platform).toMatchObject({ credits: "40000000000", duffs: 40_000_000, syncing: false });
    expect(platform?.addresses.map((a) => a.credits)).toEqual(["30000000000", "10000000000"]);
    fixture.world.pending.shielded = null;
    fixture.world.pending.platform = null;
    expect(await backend.shieldedBalance(W)).toBeNull();
    expect(await backend.platformAddresses(W)).toBeNull();
    expect(await backend.platformActivity(W)).toBeNull();
  });

  it("quotes a move to Core and refuses a grant that is too small", async () => {
    const quote = await backend.platformMoveToCoreQuote(W, { kind: "all" });
    expect(quote).toMatchObject({ credits: "40000000000", feeCredits: "2000000", expectedDuffs: 39_998_000, grant: { maxCredits: "40000000000" } });
    expect(quote.to).toMatch(/^y/);
    await expect(backend.platformMoveToCore(W, { kind: "all" }, "no-such-grant")).rejects.toMatchObject({ code: "platform.grant_invalid" });
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
