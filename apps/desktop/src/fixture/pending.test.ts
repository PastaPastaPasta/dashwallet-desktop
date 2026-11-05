// The fixture's answers for the commands dw-app does not have yet (pending.ts), through
// IpcBackend as the screens call them: the adapter's "absent" and "present" paths.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IpcBackend } from "../api/tauri";
import { installFixture, type FixtureHandle } from "./index";
import { PLATFORM_PAYEE } from "./pending";
import { FIXTURE_PASSPHRASE, FIXTURE_WALLET_ID as W } from "./scenarios";

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
    expect(await backend.shieldedStatus(W)).toMatchObject({ active: true, ready: true, balance: "125000000000" });
    const platform = await backend.platformAddresses(W);
    expect(platform).toMatchObject({ credits: "40000000000", duffs: 40_000_000, syncing: false });
    expect(platform?.addresses.map((a) => a.credits)).toEqual(["30000000000", "10000000000"]);
    fixture.world.pending.shielded = null;
    fixture.world.pending.platform = null;
    expect(await backend.shieldedStatus(W)).toBeNull();
    expect(await backend.shieldedHistory(W, 0, 10)).toBeNull();
    expect(await backend.platformAddresses(W)).toBeNull();
    expect(await backend.platformActivity(W)).toBeNull();
  });

  it("quotes a move to Core and refuses a grant that is too small", async () => {
    const quote = await backend.platformMoveToCoreQuote(W, { kind: "all" });
    expect(quote).toMatchObject({ credits: "40000000000", feeCredits: "2000000", expectedDuffs: 39_998_000, grant: { maxCredits: "40000000000" } });
    expect(quote.to).toMatch(/^y/);
    await expect(backend.platformMoveToCore(W, { kind: "all" }, quote.to, "no-such-grant")).rejects.toMatchObject({ code: "platform.grant_invalid" });
  });

  it("shows the Platform receive address and reads the addresses on request", async () => {
    expect(await backend.platformReceiveAddress(W)).toEqual(fixture.world.pending.platform!.receive);
    expect(await backend.platformAddressesSync(W)).toMatchObject({ credits: "40000000000" });
    expect(fixture.calls).toContain("platform_addresses_sync");
  });

  it("quotes a send with the fee band, Max with the fee from the amount, and refuses what the engine refuses", async () => {
    expect(await backend.platformSendQuote(W, PLATFORM_PAYEE, { kind: "credits", credits: "1000000000" })).toMatchObject({
      credits: "1019500000",
      amountCredits: "1000000000",
      feeCredits: "6500000",
      maxFeeCredits: "19500000",
      feeFromAmount: false,
      to: PLATFORM_PAYEE,
      grant: { maxDuffs: 0, maxCredits: "1019500000" },
    });
    expect(await backend.platformSendQuote(W, PLATFORM_PAYEE, { kind: "all" })).toMatchObject({ credits: "40000000000", amountCredits: "40000000000", feeFromAmount: true });
    const reason = (to: string) => backend.platformSendQuote(W, to, { kind: "all" }).catch((e: { code: string; data: { reason: string } }) => `${e.code} ${e.data.reason}`);
    expect(await reason("yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf")).toBe("credits.recipient_invalid core");
    expect(await reason("tdash1zq0k8yv6m2e3x9n4c5w7a8s0d2f4g6h8j0k2l4m6n8p0q2r4s6t8u0v2w4x6y8z0a2c4e6g8j0m2")).toBe("credits.recipient_invalid shielded");
    expect(await reason("dash1kq7r3m9x2v5c8n4z6w0e2t4y6u8p0s2a4d6f8h")).toBe("credits.recipient_invalid network");
    expect(await reason(fixture.world.pending.platform!.receive.address)).toBe("credits.recipient_invalid own");
    expect(await reason("tdash1bad!")).toBe("credits.recipient_invalid malformed");
    await expect(backend.platformSendQuote(W, PLATFORM_PAYEE, { kind: "credits", credits: "1000" })).rejects.toMatchObject({ code: "credits.send_out_of_range", data: { kind: "creditMinimum", min: "500000" } });
    await expect(backend.platformSendQuote(W, PLATFORM_PAYEE, { kind: "credits", credits: "39990000000" })).rejects.toMatchObject({ code: "platform.insufficient_credits" });
  });

  it("records a send in the activity and holds the next one while it is pending", async () => {
    fixture.world.pending.platform!.nextSend = "pending";
    const quote = await backend.platformSendQuote(W, PLATFORM_PAYEE, { kind: "credits", credits: "1000000000" });
    const grant = await backend.authorize({ kind: "platformOp", maxDuffs: 0, maxCredits: quote.grant.maxCredits }, W, { kind: "passphrase", passphrase: FIXTURE_PASSPHRASE });
    const sent = await backend.platformSend(W, PLATFORM_PAYEE, { kind: "credits", credits: "1000000000" }, grant.id);
    expect(sent).toMatchObject({ state: "pending", credits: "1019500000", to: PLATFORM_PAYEE, feeCredits: null });
    expect((await backend.platformActivity(W))?.[0]).toMatchObject({ kind: "send", state: "pending", to: PLATFORM_PAYEE });
    // The quote plans; the send refuses while one may still go through.
    const next = await backend.platformSendQuote(W, PLATFORM_PAYEE, { kind: "all" });
    const again = await backend.authorize({ kind: "platformOp", maxDuffs: 0, maxCredits: next.grant.maxCredits }, W, { kind: "passphrase", passphrase: FIXTURE_PASSPHRASE });
    await expect(backend.platformSend(W, PLATFORM_PAYEE, { kind: "all" }, again.id)).rejects.toMatchObject({ code: "credits.move_pending" });
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
