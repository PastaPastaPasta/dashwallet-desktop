// The fixture's answers for the commands dw-app does not have yet (pending.ts), through
// IpcBackend as the screens call them: the adapter's "absent" and "present" paths.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IpcBackend } from "../api/tauri";
import { installFixture, type FixtureHandle } from "./index";
import { landSend, PLATFORM_PAYEE } from "./pending";
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

  it("quotes a send with the fee bounded by what its source keeps, Max with the fee from the amount, and refuses what the engine refuses", async () => {
    // The largest address (30,000,000,000) gives 1,000,000,000 and keeps 29,000,000,000: the most the fee can take.
    expect(await backend.platformSendQuote(W, PLATFORM_PAYEE, { kind: "credits", credits: "1000000000" }, null)).toMatchObject({
      credits: "30000000000",
      amountCredits: "1000000000",
      feeCredits: "6500000",
      maxFeeCredits: "29000000000",
      feeFromAmount: false,
      to: PLATFORM_PAYEE,
      grant: { maxDuffs: 0, maxCredits: "30000000000" },
      retryOf: null,
    });
    // More than the largest address holds: the fee source is the next address that keeps enough.
    expect(await backend.platformSendQuote(W, PLATFORM_PAYEE, { kind: "credits", credits: "32000000000" }, null)).toMatchObject({ credits: "40000000000", maxFeeCredits: "8000000000" });
    expect(await backend.platformSendQuote(W, PLATFORM_PAYEE, { kind: "all" }, null)).toMatchObject({ credits: "40000000000", amountCredits: "40000000000", maxFeeCredits: null, feeFromAmount: true, retryOf: null });
    const reason = (to: string) => backend.platformSendQuote(W, to, { kind: "all" }, null).catch((e: { code: string; data: { reason: string } }) => `${e.code} ${e.data.reason}`);
    expect(await reason("yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf")).toBe("credits.recipient_invalid core");
    expect(await reason("tdash1zq0k8yv6m2e3x9n4c5w7a8s0d2f4g6h8j0k2l4m6n8p0q2r4s6t8u0v2w4x6y8z0a2c4e6g8j0m2")).toBe("credits.recipient_invalid shielded");
    expect(await reason("dash1kq7r3m9x2v5c8n4z6w0e2t4y6u8p0s2a4d6f8h")).toBe("credits.recipient_invalid network");
    expect(await reason(fixture.world.pending.platform!.receive.address)).toBe("credits.recipient_invalid own");
    expect(await reason("tdash1bad!")).toBe("credits.recipient_invalid malformed");
    await expect(backend.platformSendQuote(W, PLATFORM_PAYEE, { kind: "credits", credits: "1000" }, null)).rejects.toMatchObject({ code: "credits.send_out_of_range", data: { kind: "creditMinimum", min: "500000" } });
    await expect(backend.platformSendQuote(W, PLATFORM_PAYEE, { kind: "credits", credits: "39999000000" }, null)).rejects.toMatchObject({ code: "platform.insufficient_credits" });
  });

  const credits = (n: string) => ({ kind: "credits" as const, credits: n });
  const grantFor = (maxCredits: string) => backend.authorize({ kind: "platformOp", maxDuffs: 0, maxCredits }, W, { kind: "passphrase", passphrase: FIXTURE_PASSPHRASE });
  const pay = async (amount: { kind: "all" } | { kind: "credits"; credits: string }, to = PLATFORM_PAYEE) => {
    const quote = await backend.platformSendQuote(W, to, amount, null);
    const grant = await grantFor(quote.grant.maxCredits);
    return { quote, outcome: await backend.platformSend(W, quote.operation, grant.id) };
  };
  const platform = () => fixture.world.pending.platform!;

  it("quotes every send with its own single-use operation", async () => {
    const a = await backend.platformSendQuote(W, PLATFORM_PAYEE, credits("1000000000"), null);
    const b = await backend.platformSendQuote(W, PLATFORM_PAYEE, credits("1000000000"), null);
    expect(a.operation).toMatch(/^op-/);
    expect(b.operation).not.toBe(a.operation);
    const grant = await grantFor(a.grant.maxCredits);
    await expect(backend.platformSend(W, "op-nope", grant.id)).rejects.toMatchObject({ code: "invalid_argument" });
    expect(await backend.platformSend(W, a.operation, grant.id)).toMatchObject({ state: "done" });
    // Used: quote again.
    const again = await grantFor(a.grant.maxCredits);
    await expect(backend.platformSend(W, a.operation, again.id)).rejects.toMatchObject({ code: "invalid_argument" });
  });

  it("takes only a fresh platformOp grant for a send: a lease id, or no such grant, is grant_invalid", async () => {
    const quote = await backend.platformSendQuote(W, PLATFORM_PAYEE, credits("1000000000"), null);
    await expect(backend.platformSend(W, quote.operation, "lease-1")).rejects.toMatchObject({ code: "platform.grant_invalid" });
    const small = await grantFor("1000");
    const q2 = await backend.platformSendQuote(W, PLATFORM_PAYEE, credits("1000000000"), null);
    await expect(backend.platformSend(W, q2.operation, small.id)).rejects.toMatchObject({ code: "platform.grant_exceeded" });
  });

  it("records a send in the activity and holds the next one while it is pending", async () => {
    platform().nextSend = "pending";
    const { outcome } = await pay(credits("1000000000"));
    expect(outcome).toMatchObject({ state: "pending", mayLand: true, credits: "1000000000", amountCredits: "1000000000", to: PLATFORM_PAYEE, feeCredits: null, estimated: false });
    expect((await backend.platformActivity(W))?.[0]).toMatchObject({ kind: "send", state: "pending", mayLand: true, to: PLATFORM_PAYEE });
    // The quote refuses before any passphrase: the held send is not retried by this one.
    await expect(backend.platformSendQuote(W, PLATFORM_PAYEE, { kind: "all" }, null)).rejects.toMatchObject({ code: "credits.move_pending", data: { kind: "artifact", artifact: outcome.id } });
    // Not even the same payment while Platform has not answered it.
    await expect(backend.platformSendQuote(W, PLATFORM_PAYEE, credits("1000000000"), null)).rejects.toMatchObject({ code: "credits.move_pending" });
  });

  it("holds a send that failed by height and may land, and quotes only its exact retry (retryOf)", async () => {
    platform().nextSend = "failed";
    const { outcome } = await pay(credits("1000000000"));
    expect(outcome).toMatchObject({ state: "failed", mayLand: true });
    const other = "tdash1sq7r3m9x2v5c8n4z6w0e2t4y6u8p0s2a4d6f8h";
    for (const attempt of [backend.platformSendQuote(W, other, credits("1000000000"), null), backend.platformSendQuote(W, PLATFORM_PAYEE, credits("2000000000"), null), backend.platformSendQuote(W, PLATFORM_PAYEE, { kind: "all" }, null)])
      await expect(attempt).rejects.toMatchObject({ code: "credits.move_pending", data: { kind: "artifact", artifact: outcome.id } });
    expect(await backend.platformSendQuote(W, PLATFORM_PAYEE, credits("1000000000"), null)).toMatchObject({ retryOf: outcome.id, credits: "30000000000", amountCredits: "1000000000" });
  });

  it("refuses a new send whose hold appeared after its quote", async () => {
    const quote = await backend.platformSendQuote(W, PLATFORM_PAYEE, credits("1000000000"), null);
    platform().activity = [{ id: "pm-9", kind: "moveToCore", state: "pending", credits: "1", address: null, to: "y", expectedDuffs: null, feeCredits: null, at: 1, mayLand: true, estimated: false }, ...platform().activity];
    await expect(backend.platformSend(W, quote.operation, (await grantFor(quote.grant.maxCredits)).id)).rejects.toMatchObject({ code: "credits.move_pending", data: { artifact: "pm-9" } });
  });

  it("an exact retry supersedes the held send (replaced) and sends once", async () => {
    platform().nextSend = "failed";
    platform().thenSend = "done";
    const first = await pay(credits("1000000000"));
    const { quote, outcome } = await pay(credits("1000000000"));
    expect(quote.retryOf).toBe(first.outcome.id);
    expect(outcome).toMatchObject({ state: "done", mayLand: false, credits: "1006500000", feeCredits: "6500000" });
    const activity = (await backend.platformActivity(W))!;
    expect(activity.find((a) => a.id === first.outcome.id)).toMatchObject({ state: "replaced", mayLand: false });
    expect(activity.filter((a) => a.kind === "send" && a.state === "done")).toHaveLength(1);
    expect((await backend.platformAddresses(W))?.credits).toBe("38993500000");
    // Nothing holds any more.
    expect(await backend.platformSendQuote(W, PLATFORM_PAYEE, credits("1000000000"), null)).toMatchObject({ retryOf: null });
  });

  it("a Max retry replays the stored terms, not the balance now", async () => {
    platform().nextSend = "failed";
    const first = await pay({ kind: "all" });
    expect(first.outcome).toMatchObject({ state: "failed", feeFromAmount: true, amountCredits: "40000000000" });
    platform().addresses = [...platform().addresses, { address: "tdash1extra", credits: "5000000000" }];
    const retry = await backend.platformSendQuote(W, PLATFORM_PAYEE, { kind: "all" }, null);
    expect(retry).toMatchObject({ retryOf: first.outcome.id, amountCredits: "40000000000", credits: "40000000000", maxFeeCredits: null, feeFromAmount: true });
  });

  it("a retry of a send that landed meanwhile returns that send's outcome and debits nothing", async () => {
    platform().nextSend = "failed";
    const first = await pay(credits("1000000000"));
    const retry = await backend.platformSendQuote(W, PLATFORM_PAYEE, credits("1000000000"), null);
    const grant = await grantFor(retry.grant.maxCredits);
    landSend(platform(), first.outcome.id);
    const before = (await backend.platformAddresses(W))!.credits;
    const got = await backend.platformSend(W, retry.operation, grant.id);
    expect(got).toMatchObject({ id: first.outcome.id, state: "done", mayLand: false });
    expect((await backend.platformAddresses(W))!.credits).toBe(before);
    expect((await backend.platformActivity(W))!.filter((a) => a.kind === "send")).toHaveLength(1);
  });

  it("a quote naming retryOf is that send's retry whatever it became: a landed one is answered, nothing debited", async () => {
    platform().nextSend = "failed";
    const first = await pay(credits("1000000000"));
    landSend(platform(), first.outcome.id);
    const retry = await backend.platformSendQuote(W, PLATFORM_PAYEE, credits("1000000000"), first.outcome.id);
    expect(retry).toMatchObject({ retryOf: first.outcome.id });
    const before = (await backend.platformAddresses(W))!.credits;
    expect(await backend.platformSend(W, retry.operation, (await grantFor(retry.grant.maxCredits)).id)).toMatchObject({ id: first.outcome.id, state: "done", mayLand: false });
    expect((await backend.platformAddresses(W))!.credits).toBe(before);
    expect((await backend.platformActivity(W))!.filter((a) => a.kind === "send")).toHaveLength(1);
    // Without the name, the same payment is a new one.
    expect(await backend.platformSendQuote(W, PLATFORM_PAYEE, credits("1000000000"), null)).toMatchObject({ retryOf: null });
  });

  it("a named retry of a replaced send answers the retry that landed in its place", async () => {
    platform().nextSend = "failed";
    platform().thenSend = "done";
    const first = await pay(credits("1000000000"));
    const second = await pay(credits("1000000000"));
    const before = (await backend.platformAddresses(W))!.credits;
    const named = await backend.platformSendQuote(W, PLATFORM_PAYEE, credits("1000000000"), first.outcome.id);
    expect(await backend.platformSend(W, named.operation, (await grantFor(named.grant.maxCredits)).id)).toMatchObject({ id: second.outcome.id, state: "done" });
    expect((await backend.platformAddresses(W))!.credits).toBe(before);
  });

  it("a named retry of a send that failed for good returns it failed with mayLand false, signing nothing", async () => {
    platform().nextSend = "failed";
    const first = await pay(credits("1000000000"));
    platform().activity = platform().activity.map((a) => (a.id === first.outcome.id ? { ...a, mayLand: false } : a));
    const named = await backend.platformSendQuote(W, PLATFORM_PAYEE, credits("1000000000"), first.outcome.id);
    expect(await backend.platformSend(W, named.operation, (await grantFor(named.grant.maxCredits)).id)).toMatchObject({ id: first.outcome.id, state: "failed", mayLand: false });
    expect((await backend.platformActivity(W))!.filter((a) => a.kind === "send")).toHaveLength(1);
  });

  it("a named retry with another recipient or amount, or of an unknown send, is invalid_argument", async () => {
    platform().nextSend = "failed";
    const first = await pay(credits("1000000000"));
    const id = first.outcome.id;
    await expect(backend.platformSendQuote(W, PLATFORM_PAYEE, credits("2000000000"), id)).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(backend.platformSendQuote(W, PLATFORM_PAYEE, { kind: "all" }, id)).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(backend.platformSendQuote(W, "tdash1sq7r3m9x2v5c8n4z6w0e2t4y6u8p0s2a4d6f8h", credits("1000000000"), id)).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(backend.platformSendQuote(W, PLATFORM_PAYEE, credits("1000000000"), "ps-nope")).rejects.toMatchObject({ code: "invalid_argument" });
  });

  it("an unsettled send's row is its amount, no band; a done one is the amount and the fee", async () => {
    platform().nextSend = "failed";
    const held = await pay(credits("1000000000"));
    expect((await backend.platformActivity(W))![0]).toMatchObject({ id: held.outcome.id, credits: "1000000000", feeCredits: null });
    landSend(platform(), held.outcome.id);
    expect((await backend.platformActivity(W))![0]).toMatchObject({ credits: "1006500000", feeCredits: "6500000", state: "done" });
  });

  it("lets a move follow a held move (the engine signs it at the same inputs) and replaces it", async () => {
    fixture = installFixture({ scenario: "platform-move-failed-may-land" });
    backend = new IpcBackend("fixture");
    const quote = await backend.platformMoveToCoreQuote(W, { kind: "all" });
    expect(await backend.platformMoveToCore(W, { kind: "all" }, quote.to, (await grantFor(quote.grant.maxCredits)).id)).toMatchObject({ state: "done" });
    const activity = (await backend.platformActivity(W))!;
    expect(activity.find((a) => a.id === "pm-held")).toMatchObject({ state: "replaced", mayLand: false });
    expect(activity.some((a) => a.kind === "moveToCore" && a.mayLand)).toBe(false);
  });

  it("refuses a move beside a move Platform has not answered", async () => {
    platform().nextMove = "pending";
    const quote = await backend.platformMoveToCoreQuote(W, { kind: "all" });
    const first = await backend.platformMoveToCore(W, { kind: "all" }, quote.to, (await grantFor(quote.grant.maxCredits)).id);
    expect(first.state).toBe("pending");
    await expect(backend.platformMoveToCore(W, { kind: "all" }, quote.to, (await grantFor(quote.grant.maxCredits)).id)).rejects.toMatchObject({ code: "credits.move_pending", data: { artifact: first.id } });
  });

  it("a done send that hid its fee counts the estimate", async () => {
    platform().estimateSends = true;
    const { outcome } = await pay(credits("1000000000"));
    expect(outcome).toMatchObject({ state: "done", estimated: true, feeCredits: null, credits: "1006500000" });
    expect((await backend.platformActivity(W))?.[0]).toMatchObject({ estimated: true, mayLand: false });
  });

  it("holds a move beside a send that may still land", async () => {
    platform().nextSend = "failed";
    const { outcome } = await pay(credits("1000000000"));
    const quote = await backend.platformMoveToCoreQuote(W, { kind: "all" });
    const grant = await grantFor(quote.grant.maxCredits);
    await expect(backend.platformMoveToCore(W, { kind: "all" }, quote.to, grant.id)).rejects.toMatchObject({ code: "credits.move_pending", data: { artifact: outcome.id } });
  });

  it("seeds a send that may land for the scenario: its row carries mayLand and its exact retry is quoted", async () => {
    fixture = installFixture({ scenario: "platform-send-failed-may-land" });
    backend = new IpcBackend("fixture");
    expect((await backend.platformActivity(W))?.[0]).toMatchObject({ id: "ps-held", kind: "send", state: "failed", mayLand: true });
    expect(await backend.platformSendQuote(W, PLATFORM_PAYEE, credits("1000000000"), null)).toMatchObject({ retryOf: "ps-held" });
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
