// The fixture's send engine (send.ts, U-03 D18), through the generated commands as the Pay
// screen calls them: the estimate and its refusals, the payment-authentication rule, the spend
// grant's cap, the broadcast outcomes and handles (E0-13 D8), the transaction actions, Receive.

import { clearMocks } from "@tauri-apps/api/mocks";
import { listen } from "@tauri-apps/api/event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import type { AppEvent } from "../api/generated/commands";
import type { Recipient } from "../api/generated/Recipient";
import { installFixture, type FixtureHandle } from "./index";
import { FIXTURE_PASSPHRASE, FIXTURE_WALLET_ID as W } from "./scenarios";
import { classifyAddress, paymentUri, parseAmount, qrMatrix } from "./send";

const PAYEE = "yXdjeVZqmWp4ZcVJ2Hh8hcFhUzFE8wD1Cd";
const PASS = { kind: "passphrase", passphrase: FIXTURE_PASSPHRASE } as const;
const pay = (amount: number, extra: Partial<Recipient> = {}): Recipient => ({ address: PAYEE, amount, subtractFeeFromAmount: false, label: null, message: null, ...extra });

let fixture: FixtureHandle;
beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  fixture = installFixture({ scenario: "funded" });
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

async function draftFor(r: Recipient) {
  const d = await cmd.newTxDraft(W);
  await cmd.draftSetRecipients(d, [r]);
  return d;
}

async function prepared(r: Recipient) {
  const d = await draftFor(r);
  const est = await cmd.draftEstimate(d);
  const grant = await cmd.authorize({ kind: "spend", maxDuffs: est.totalSent + est.fee }, W, PASS);
  return { d, est, p: await cmd.draftPrepare(d, grant.id) };
}

describe("fixture send engine", () => {
  it("estimates with the largest coin first and a 1 000 duffs/kB fee", async () => {
    const est = await cmd.draftEstimate(await draftFor(pay(50_000_000)));
    // One input, the payment and change: 10 + 148 + 2 × 34 = 226 bytes.
    expect(est).toEqual({ fee: 226, sizeBytes: 226, inputCount: 1, change: 2_000_000_000 - 50_000_000 - 226, totalSent: 50_000_000 });
    const sub = await cmd.draftEstimate(await draftFor(pay(50_000_000, { subtractFeeFromAmount: true })));
    expect(sub.totalSent).toBe(50_000_000 - 226);
  });

  it("refuses as dw-engine does", async () => {
    const refusal = async (r: Recipient) => (await cmd.draftEstimate(await draftFor(r)).catch((e: cmd.AppError) => e)) as cmd.AppError;
    expect((await refusal(pay(100))).code).toBe("send.dust_amount");
    expect((await refusal({ ...pay(1000), address: "Xnot-base58" })).code).toBe("send.invalid_address");
    expect((await refusal({ ...pay(1000), address: "tdash1kqqqqqqqqqqqq" })).code).toBe("send.platform_address");
    expect(await refusal(pay(5_000_000_000))).toMatchObject({ code: "send.amount_exceeds_balance", data: { kind: "duffs", available: 3_019_698_287 } });
    expect(await refusal(pay(3_019_698_287))).toMatchObject({ code: "send.amount_with_fee_exceeds_balance", data: { kind: "fee" } });
    const mixed = await draftFor(pay(1000));
    await cmd.draftSetSource(mixed, { kind: "fullyMixedOnly" });
    expect(await cmd.draftEstimate(mixed).catch((e) => e)).toMatchObject({ code: "send.insufficient_mixed_funds", data: { kind: "duffs", available: 0 } });
    expect(await cmd.draftEstimate(99).catch((e) => e)).toMatchObject({ code: "app.unknown_handle" });
  });

  it("requires a passphrase for a spend grant while every payment needs authentication", async () => {
    await expect(cmd.authorize({ kind: "spend", maxDuffs: 1 }, W, { kind: "unencrypted" })).rejects.toMatchObject({ code: "vault.credential_required" });
    fixture.world.paymentAuth = false;
    await expect(cmd.authorize({ kind: "spend", maxDuffs: 1 }, W, { kind: "unencrypted" })).resolves.toMatchObject({ walletId: W });
  });

  it("signs only within the spend grant, once", async () => {
    const d = await draftFor(pay(50_000_000));
    const small = await cmd.authorize({ kind: "spend", maxDuffs: 50_000_000 }, W, PASS);
    expect(await cmd.draftPrepare(d, small.id).catch((e) => e)).toMatchObject({
      code: "send.grant_exceeded",
      data: { kind: "spendCap", maxDuffs: 50_000_000, outflow: 50_000_226 },
    });
    // The grant was spent by the refused call.
    expect(await cmd.draftPrepare(d, small.id).catch((e) => e)).toMatchObject({ code: "send.grant_invalid" });
  });

  it("reserves the inputs, broadcasts, adds the row and moves the balance", async () => {
    const seen: AppEvent[] = [];
    await listen<AppEvent>(cmd.ENGINE_EVENT, (e) => seen.push(e.payload));
    const { d, p } = await prepared(pay(50_000_000, { label: "Alice" }));
    expect(p.summary).toMatchObject({ totalSent: 50_000_000, totalDebit: 50_000_226, fee: 226 });
    expect(p.summary.outputs.map((o) => o.isChange)).toEqual([false, true]);
    expect((await cmd.utxos(W, { includeLocked: true, fullyMixedOnly: false, minConfirmations: null })).filter((u) => u.reserved)).toHaveLength(1);
    const { txid } = await cmd.draftBroadcast(d, p.handle);
    expect(txid).toBe(p.summary.txid);
    const page = await cmd.historyPage(W, { categories: [], text: null, sort: "newestFirst", cursor: null, limit: 1 });
    expect(page.rows[0]).toMatchObject({ txid, amount: -50_000_226, fee: 226, label: "Alice", category: "sent" });
    expect((await cmd.balance(W))?.confirmed).toBe(3_019_698_287 - 50_000_226);
    expect(seen.map((e) => e.kind)).toEqual(["balances", "historyChanged"]);
    // The handle is dropped once the network accepted it.
    expect(await cmd.draftBroadcast(d, p.handle).catch((e) => e)).toMatchObject({ code: "app.unknown_handle" });
  });

  it("keeps the handle after broadcast_unknown and drops it after a rejection", async () => {
    fixture.world.send.broadcast = "unknown";
    const { d, p } = await prepared(pay(1_000_000));
    await expect(cmd.draftBroadcast(d, p.handle)).rejects.toMatchObject({ code: "send.broadcast_unknown" });
    fixture.world.send.broadcast = "rejected";
    await expect(cmd.draftBroadcast(d, p.handle)).rejects.toMatchObject({ code: "send.broadcast_rejected", data: { kind: "reason" } });
    await expect(cmd.draftBroadcast(d, p.handle)).rejects.toMatchObject({ code: "app.unknown_handle" });
    // The rejected transaction's inputs are free again.
    expect((await cmd.utxos(W, { includeLocked: true, fullyMixedOnly: false, minConfirmations: null })).some((u) => u.reserved)).toBe(false);
  });

  it("abandons its own unconfirmed send and gives the coins back", async () => {
    const { d, p } = await prepared(pay(1_000_000));
    const { txid } = await cmd.draftBroadcast(d, p.handle);
    expect(await cmd.txDetailExtras(W, txid)).toMatchObject({ canAbandon: true, canResend: true, abandoned: false });
    await cmd.resendTransaction(W, txid);
    expect((await cmd.txDetailExtras(W, txid)).lastAnnouncedAt).not.toBeNull();
    await cmd.abandonTransaction(W, txid);
    expect(await cmd.txDetailExtras(W, txid)).toMatchObject({ canAbandon: false, abandoned: true });
    expect((await cmd.balance(W))?.confirmed).toBe(3_019_698_287);
    await expect(cmd.abandonTransaction(W, txid)).rejects.toMatchObject({ code: "tx_action_refused", data: { kind: "txAction", refusal: "alreadyAbandoned" } });
  });

  it("labels a transaction", async () => {
    const [row] = (await cmd.historyPage(W, { categories: [], text: null, sort: "newestFirst", cursor: null, limit: 1 })).rows;
    await cmd.setTxLabel(W, row.txid, "  Lunch ");
    expect((await cmd.txDetail(W, row.txid)).label).toBe("Lunch");
  });

  it("issues receive addresses and keeps requests", async () => {
    const current = await cmd.currentReceiveAddress(W);
    expect(await cmd.currentReceiveAddress(W)).toEqual(current);
    const next = await cmd.nextReceiveAddress(W, null);
    expect(next.address).not.toBe(current.address);
    const r = await cmd.createReceiveRequest(W, 25_000_000, "Rent", null);
    expect(r.uri).toBe(`dash:${r.address}?amount=0.25&label=Rent`);
    expect(await cmd.receiveRequests(W)).toEqual([r]);
    await cmd.deleteReceiveRequest(W, r.id);
    expect(await cmd.receiveRequests(W)).toEqual([]);
    await expect(cmd.deleteReceiveRequest(W, r.id)).rejects.toMatchObject({ code: "request_not_found" });
  });
});

describe("fixture text helpers", () => {
  it("classifies the fixture's address shapes", () => {
    expect(classifyAddress("testnet", PAYEE)).toEqual({ kind: "core", scriptHash: false });
    expect(classifyAddress("mainnet", PAYEE)).toMatchObject({ kind: "invalid", problem: { kind: "invalidBase58Prefix" } });
    expect(classifyAddress("testnet", "y123")).toMatchObject({ kind: "invalid", problem: { kind: "invalidBase58Length" } });
    expect(classifyAddress("testnet", "hello world")).toMatchObject({ kind: "invalid", problem: { kind: "notBech32mOrBase58" } });
  });

  it("parses amounts as BitcoinUnits::parse", () => {
    expect(parseAmount("0.25", "dash")).toBe(25_000_000);
    expect(parseAmount("1 000.5", "dash")).toBe(100_050_000_000);
    expect(parseAmount(".5", "dash")).toBe(50_000_000);
    expect(parseAmount("0.123456789", "dash")).toBeNull();
    expect(parseAmount("-1", "dash")).toBeNull();
    expect(parseAmount("1.2.3", "dash")).toBeNull();
    expect(parseAmount("", "dash")).toBeNull();
  });

  it("writes dash: URIs and refuses a QR code over 255 characters", () => {
    expect(paymentUri(PAYEE, null, "A B", "x&y")).toBe(`dash:${PAYEE}?label=A%20B&message=x%26y`);
    const qr = qrMatrix(paymentUri(PAYEE, 100_000_000, null, null));
    expect(qr.modules).toHaveLength(qr.size * qr.size);
    expect(() => qrMatrix("x".repeat(256))).toThrow();
  });
});
