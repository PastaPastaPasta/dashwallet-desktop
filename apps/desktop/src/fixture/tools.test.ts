// The fixture's Tools (tools.ts, U-05), through the generated commands as the Tools screens call
// them: the console's grant round trip and redaction, Repair's refusals and rescan progress, the
// address book's codes, signing and verifying, PSBTs, Create Unsigned, and coin locks.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import type { AppError } from "../api/generated/commands";
import { installFixture, type FixtureHandle } from "./index";
import { dashpayInfo, type DashPayInfoState } from "../tools/dashpayInfo";
import { FIXTURE_PASSPHRASE, FIXTURE_WALLET_ID as W, type ScenarioName } from "./scenarios";
import { FIXTURE_PSBT } from "./tools";

const PASS = { kind: "passphrase", passphrase: FIXTURE_PASSPHRASE } as const;
const PAYEE = "yXdjeVZqmWp4ZcVJ2Hh8hcFhUzFE8wD1Cd";
const code = (p: Promise<unknown>) => p.then(() => "ok", (e: AppError) => e.code);

let fixture: FixtureHandle;
beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  fixture = installFixture({ scenario: "funded" });
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

describe("fixture Information", () => {
  it("SPV has no mempool, and the tip follows sync", async () => {
    const info = await cmd.nodeInfo();
    expect(info).toMatchObject({ mempoolTxCount: null, mempoolUsageBytes: null, connectionsOut: 8, network: { dirName: "testnet" } });
    expect(info.tipHeight).toBe((await cmd.syncStatus()).tipHeight);
    expect(await cmd.warnings()).toEqual([]);
  });
});

describe("fixture DashPay card (DP6-03)", () => {
  /** The card's state for a scenario, read as the screen reads it. */
  async function cardState(scenario: ScenarioName) {
    clearMocks();
    installFixture({ scenario });
    const [status, sync, trustedOnly] = await Promise.all([cmd.dashpayStatus(W), cmd.dashpaySyncStatus(W), cmd.trustedQuorumsOnly()]);
    return dashpayInfo({ noWallet: false, status, sync, syncError: null, quorumSource: undefined, trustedOnly, now: 0 }).state;
  }

  it("each state has its scenario", async () => {
    const expected: [ScenarioName, DashPayInfoState][] = [
      ["dashpay-info-spv", "running"],
      ["dashpay-registered", "degraded"],
      ["dashpay-info-fallback", "degraded"],
      ["dashpay-info-trusted-only", "degraded"],
      ["dashpay-info-pass-failed", "failed"],
      ["dashpay-look-failed", "failed"],
      ["dashpay-starting", "starting"],
      ["dashpay-identity-unsettled", "starting"],
      ["dashpay-not-started", "notStarted"],
      ["funded", "notSetUp"],
      ["dashpay-waiting-for-sync", "notSetUp"],
      ["dashpay-watch-only", "watchOnly"],
      ["dashpay-info-stopped", "running"],
    ];
    for (const [scenario, state] of expected) expect(await cardState(scenario), scenario).toBe(state);
  });

  it("reports the engine's fields: the bring-up, its queue, the loops, the session's quorum source", async () => {
    expect(await cmd.dashpaySyncStatus(W)).toMatchObject({ startup: "noIdentity", lastPass: null, pendingContactCrypto: 0, quorumSource: "trusted" });
    expect(await cmd.quorumSource()).toBe("trusted");
    expect(await cmd.trustedQuorumsOnly()).toBe(false);
    const loops = (await cmd.dashpaySyncStatus(W)).loops;
    expect(loops.map((l) => [l.syncLoop, l.running])).toEqual([
      ["identitySync", true],
      ["dashPaySync", true],
      ["dpnsSync", true],
    ]);
    // No bring-up: nothing ran, nothing runs.
    clearMocks();
    installFixture({ scenario: "dashpay-not-started" });
    expect((await cmd.dashpaySyncStatus(W)).loops.every((l) => !l.running && l.lastRunAt === null)).toBe(true);
    // SPV stopped: the loops stop, and the card holds Sync now.
    clearMocks();
    installFixture({ scenario: "dashpay-info-stopped" });
    expect((await cmd.dashpaySyncStatus(W)).loops.every((l) => !l.running && l.lastRunAt !== null)).toBe(true);
  });
});

describe("fixture console", () => {
  it("runs a line, and redacts a sensitive one", async () => {
    expect(await cmd.consoleExecute(null, "getblockcount", null)).toMatchObject({ isJson: false, filtered: "getblockcount" });
    expect(await cmd.consoleExecute(null, "getnetworkinfo", null)).toMatchObject({ isJson: true });
    expect(await cmd.consoleRedact('walletpassphrase "secret" 60')).toBe("walletpassphrase(…)");
    expect(await cmd.consoleRedact("help walletpassphrase")).toBe("help walletpassphrase");
    expect(await code(cmd.consoleRedact('walletpassphrase "unbalanced'))).toBe("console.parse_error");
  });

  it("refuses unknown, unavailable and walletless lines with the engine's codes", async () => {
    expect(await code(cmd.consoleExecute(null, "stop", null))).toBe("console.not_available");
    expect(await code(cmd.consoleExecute(null, "frobnicate", null))).toBe("console.not_available");
    expect(await code(cmd.consoleExecute(null, "getbalance", null))).toBe("console.wallet_required");
    expect(await code(cmd.consoleExecute(null, "getblockcount 1", null))).toBe("console.rpc_error");
    expect((await cmd.consoleExecute(W, "getbalance", null)).text).toMatch(/^\d+\.\d{8}$/);
  });

  it("signmessage asks for a grant, then spends the one it is given", async () => {
    const coin = (await cmd.utxos(W, { includeLocked: true, fullyMixedOnly: false, minConfirmations: null }))[0];
    const line = `signmessage ${coin.address} hello`;
    const e = await cmd.consoleExecute(W, line, null).catch((x: AppError) => x);
    expect(e).toMatchObject({ code: "console.authorization_required", data: { kind: "authorize", purpose: { kind: "signMessage" }, walletId: W } });
    const grant = await cmd.authorize({ kind: "signMessage" }, W, PASS);
    const signed = await cmd.consoleExecute(W, line, grant.id);
    expect((await cmd.consoleExecute(null, `verifymessage ${coin.address} ${signed.text} hello`, null)).text).toBe("true");
    expect(await code(cmd.consoleExecute(W, line, grant.id))).toBe("vault.grant_invalid");
  });
});

describe("fixture Peers and Repair", () => {
  it("rotates peers, and has none while SPV is stopped", async () => {
    const before = await cmd.peers();
    await cmd.rotatePeers();
    expect((await cmd.peers()).map((p) => p.address)).not.toEqual(before.map((p) => p.address));
    await cmd.stopSpv();
    expect(await cmd.peers()).toEqual([]);
    expect(await code(cmd.rotatePeers())).toBe("no_peers");
  });

  it("rescans with progress until the tip, refuses a second one, and cancels", async () => {
    const tip = (await cmd.syncStatus()).tipHeight!;
    expect(await code(cmd.rescan({ kind: "height", height: tip + 1 }))).toBe("height_out_of_range");
    await cmd.rescan({ kind: "height", height: tip - 25_000 });
    expect(await code(cmd.rescan({ kind: "genesis" }))).toBe("rescan_in_progress");
    const seen: number[] = [];
    for (let p = await cmd.rescanProgress(); p; p = await cmd.rescanProgress()) seen.push(p.currentHeight!);
    expect(seen).toEqual([tip - 25_000, tip - 15_000, tip - 5_000]);
    await cmd.rescan({ kind: "walletBirth" });
    expect(await cmd.cancelRescan()).toBe(true);
    expect(await cmd.rescanProgress()).toBeNull();
    expect(await cmd.cancelRescan()).toBe(false);
  });

  it("resets chain data only while SPV is stopped", async () => {
    expect(await code(cmd.resetChainData())).toBe("spv_running");
    await cmd.stopSpv();
    expect(await cmd.spvRunning()).toBe(false);
    await cmd.resetChainData();
    await cmd.startSpv();
    expect(await cmd.spvRunning()).toBe(true);
  });

  it("a lower birth height schedules a rescan from it (IOS-113, IOS-034)", async () => {
    await cmd.setBirthHeight(W, 1_100_000);
    expect(await cmd.rescanProgress()).toMatchObject({ fromHeight: 1_100_000 });
    await cmd.cancelRescan();
    expect(await cmd.dropUnconfirmed(null)).toBe(1);
  });
});

describe("fixture address book", () => {
  it("lists, adds, edits and deletes with the engine's codes", async () => {
    const send = await cmd.addressBook(W, "send", null);
    const receive = await cmd.addressBook(W, "receive", null);
    expect(send.map((e) => e.label)).toEqual(["Alice", "Bob's shop"]);
    expect(receive.map((e) => e.label)).toEqual(["Payroll"]);
    expect(await code(cmd.saveAddressBookEntry(W, "nonsense", "x", "send", false))).toBe("labels.invalid_address");
    expect(await code(cmd.saveAddressBookEntry(W, send[0].address, "x", "send", false))).toBe("labels.duplicate_address");
    expect(await code(cmd.saveAddressBookEntry(W, PAYEE, "x", "send", true))).toBe("labels.entry_not_found");
    await cmd.saveAddressBookEntry(W, PAYEE, "Carol", "send", false);
    await cmd.saveAddressBookEntry(W, receive[0].address, "Salary", "receive", true);
    expect(await code(cmd.deleteAddressBookEntry(W, receive[0].address))).toBe("labels.receive_entry_not_deletable");
    await cmd.deleteAddressBookEntry(W, send[1].address);
    expect((await cmd.addressBook(W, null, null)).map((e) => e.label)).toEqual(["Alice", "Salary", "Carol"]);
  });

  it("refuses the wallet's own address as a sending entry", async () => {
    const own = (await cmd.utxos(W, { includeLocked: true, fullyMixedOnly: false, minConfirmations: null }))[1].address;
    expect(await code(cmd.saveAddressBookEntry(W, own, "me", "send", false))).toBe("labels.own_address");
  });
});

describe("fixture Sign/Verify", () => {
  it("signs with a grant, and verifies with dash-qt's outcomes", async () => {
    const address = (await cmd.utxos(W, { includeLocked: true, fullyMixedOnly: false, minConfirmations: null }))[0].address;
    const grant = await cmd.authorize({ kind: "signMessage" }, W, PASS);
    const sig = await cmd.signMessage(W, address, "hello", grant.id);
    expect(await cmd.verifyMessage(address, sig, "hello")).toEqual({ outcome: "verified", text: "Message verified." });
    expect((await cmd.verifyMessage(address, sig, "hellO")).outcome).toBe("notSigned");
    expect((await cmd.verifyMessage(address, "!!", "hello")).outcome).toBe("malformedSignature");
    expect((await cmd.verifyMessage("8AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", sig, "hello")).outcome).toBe("addressNoKey");
    expect(await code(cmd.signMessage(W, address, "again", grant.id))).toBe("vault.grant_invalid");
    const other = await cmd.authorize({ kind: "signMessage" }, W, PASS);
    expect(await code(cmd.signMessage(W, PAYEE, "x", other.id))).toBe("address_not_mine");
  });
});

describe("fixture PSBT", () => {
  it("analyzes, signs under a spend grant covering the total, and broadcasts", async () => {
    const a = await cmd.analyzePsbt(W, FIXTURE_PSBT.unsigned);
    expect(a).toMatchObject({ status: "needsSignatures", signability: "canSign", unsignedInputs: 2, fee: 226, total: 150_000_226 });
    expect((await cmd.analyzePsbt(null, FIXTURE_PSBT.unsigned)).signability).toBe("noWallet");
    expect((await cmd.analyzePsbt(W, FIXTURE_PSBT.foreign)).signability).toBe("noMatchingKeys");
    expect(await code(cmd.analyzePsbt(W, "bm90IGEgcHNidA=="))).toBe("psbt.invalid");
    expect(await code(cmd.broadcastPsbt(FIXTURE_PSBT.unsigned))).toBe("psbt.not_complete");

    const short = await cmd.authorize({ kind: "spend", maxDuffs: a.total! - 1 }, W, PASS);
    expect(await code(cmd.signPsbt(W, FIXTURE_PSBT.unsigned, short.id))).toBe("psbt.grant_exceeded");
    const grant = await cmd.authorize({ kind: "spend", maxDuffs: a.total! }, W, PASS);
    const signed = await cmd.signPsbt(W, FIXTURE_PSBT.unsigned, grant.id);
    expect(await cmd.analyzePsbt(W, signed)).toMatchObject({ status: "complete", unsignedInputs: 0 });
    expect(await cmd.broadcastPsbt(signed)).toMatch(/^[0-9a-f]{64}$/);
  });

  it("Create Unsigned makes a PSBT of the draft that the PSBT tab can read (QT-077)", async () => {
    const d = await cmd.newTxDraft(W);
    await cmd.draftSetRecipients(d, [{ address: PAYEE, amount: 100_000_000, subtractFeeFromAmount: false, label: null, message: null }]);
    const est = await cmd.draftEstimate(d);
    const psbt = await cmd.draftCreateUnsigned(d);
    const a = await cmd.analyzePsbt(W, psbt);
    expect(a.outputs[0]).toEqual({ address: PAYEE, amount: 100_000_000, isMine: false });
    expect(a).toMatchObject({ status: "needsSignatures", signability: "canSign", fee: est.fee, total: 100_000_000 + est.fee });
    // Nothing is reserved.
    const coins = await cmd.utxos(W, { includeLocked: true, fullyMixedOnly: false, minConfirmations: null });
    expect(coins.some((u) => u.reserved)).toBe(false);
  });
});

describe("fixture coin locks (QT-070)", () => {
  it("a locked coin is not spendable until unlocked", async () => {
    const all = { includeLocked: true, fullyMixedOnly: false, minConfirmations: null };
    const [coin] = await cmd.utxos(W, all);
    await cmd.lockOutpoints(W, [coin.outpoint]);
    expect((await cmd.utxos(W, all))[0]).toMatchObject({ userLocked: true, spendable: false });
    expect(await cmd.utxos(W, { ...all, includeLocked: false })).toHaveLength(4);
    await cmd.unlockOutpoints(W, [coin.outpoint]);
    expect((await cmd.utxos(W, all))[0]).toMatchObject({ userLocked: false, spendable: true });
  });

  it("injected failures still reach the screen", async () => {
    fixture.fail("lock_outpoints", "none");
    expect(await code(cmd.lockOutpoints(W, []))).not.toBe("ok");
  });
});

describe("fixture Tools scenarios", () => {
  it("tools-stuck-funding: resolving spends a spend grant once; a second try is refused", async () => {
    clearMocks();
    installFixture({ scenario: "tools-stuck-funding" });
    expect((await cmd.registrations(W))[0].waiting).toBe("fundingUnknown");
    const grant = await cmd.authorize({ kind: "spend", maxDuffs: 10_000_000 }, W, PASS);
    expect((await cmd.resolveStuckFunding(W, "registration/reg-1/funding", grant.id)).txid).toMatch(/^[0-9a-f]{64}$/);
    expect((await cmd.registrations(W))[0].waiting).toBeNull();
    const again = await cmd.authorize({ kind: "spend", maxDuffs: 10_000_000 }, W, PASS);
    const e = await cmd.resolveStuckFunding(W, "registration/reg-1/funding", again.id).catch((x: AppError) => x);
    expect(e).toMatchObject({ code: "stuck_funding", data: { kind: "stuckFunding", refusal: { kind: "notStuck" } } });
  });

  it("tools-warnings and tools-coinjoin", async () => {
    clearMocks();
    installFixture({ scenario: "tools-warnings" });
    expect((await cmd.warnings()).map((x) => x.code)).toContain("uncleanShutdown");
    clearMocks();
    installFixture({ scenario: "tools-coinjoin" });
    const coins = await cmd.utxos(W, { includeLocked: true, fullyMixedOnly: false, minConfirmations: null });
    const mixed = coins.filter((u) => u.coinjoinDenominated);
    expect(mixed).toHaveLength(3);
    const [summary] = await cmd.listWallets();
    expect(summary.balances?.coinjoin).toBe(mixed.reduce((n, u) => n + u.amount, 0));
  });
});
