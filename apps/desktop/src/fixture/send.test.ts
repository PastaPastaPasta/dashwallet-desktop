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
import { registered } from "./dashpay";
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

describe("fixture receive username (DP3-03)", () => {
  it("is the main name, and a stored request follows it when read", async () => {
    expect(await cmd.receiveUsername(W)).toBeNull();
    const plain = await cmd.createReceiveRequest(W, null, "Rent", null);
    expect(plain).toMatchObject({ username: null, uri: `dash:${plain.address}?label=Rent` });
    fixture.world.dashpay = registered();
    expect(await cmd.receiveUsername(W)).toBe("alice");
    const [stored] = await cmd.receiveRequests(W);
    expect(stored).toMatchObject({ id: plain.id, username: "alice", uri: `dash:${plain.address}?label=Rent&user=alice` });
    fixture.world.dashpay.identities[0].namesUpdating = true;
    expect(await cmd.receiveUsername(W)).toBeNull();
    await expect(cmd.receiveUsername("ff".repeat(32))).rejects.toMatchObject({ code: "wallet_not_found" });
  });

  it("stores no request whose amount is out of range", async () => {
    await expect(cmd.createReceiveRequest(W, -1, "Rent", null)).rejects.toMatchObject({ code: "invalid_argument" });
    expect(await cmd.receiveRequests(W)).toEqual([]);
  });
});

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
    await expect(cmd.authorize({ kind: "platformOp", maxDuffs: 1, maxCredits: "1" }, W, { kind: "unencrypted" })).rejects.toMatchObject({
      code: "vault.credential_required",
    });
    fixture.world.paymentAuth = false;
    await expect(cmd.authorize({ kind: "spend", maxDuffs: 1 }, W, { kind: "unencrypted" })).resolves.toMatchObject({ walletId: W });
    // A locked vault answers that it is locked (`check_payment_auth` skips it).
    fixture.world.paymentAuth = true;
    fixture.world.lock = { ...fixture.world.lock, state: "locked" };
    await expect(cmd.authorize({ kind: "spend", maxDuffs: 1 }, W, { kind: "unencrypted" })).rejects.toMatchObject({ code: "vault.locked" });
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

  it("drops the handle and frees the inputs after a first rejection", async () => {
    fixture.world.send.broadcast = "rejected";
    const { d, p } = await prepared(pay(1_000_000));
    await expect(cmd.draftBroadcast(d, p.handle)).rejects.toMatchObject({ code: "send.broadcast_rejected", data: { kind: "reason" } });
    await expect(cmd.draftBroadcast(d, p.handle)).rejects.toMatchObject({ code: "app.unknown_handle" });
    expect((await cmd.utxos(W, { includeLocked: true, fullyMixedOnly: false, minConfirmations: null })).some((u) => u.reserved)).toBe(false);
  });

  it("never releases a transaction once its outcome was unknown", async () => {
    const reserved = async () => (await cmd.utxos(W, { includeLocked: true, fullyMixedOnly: false, minConfirmations: null })).filter((u) => u.reserved);
    fixture.world.send.broadcast = "unknown";
    const { d, p } = await prepared(pay(1_000_000));
    await expect(cmd.draftBroadcast(d, p.handle)).rejects.toMatchObject({ code: "send.broadcast_unknown" });
    // The wallet recorded it, as the engine does for a transaction it handed over.
    const [row] = (await cmd.historyPage(W, { categories: [], text: null, sort: "newestFirst", cursor: null, limit: 1 })).rows;
    expect(row).toMatchObject({ txid: p.summary.txid, amount: -p.summary.totalDebit });
    // A retry that fails stays unknown; the handle stays, abandon is refused.
    fixture.world.send.broadcast = "noPeers";
    await expect(cmd.draftBroadcast(d, p.handle)).rejects.toMatchObject({ code: "send.broadcast_unknown" });
    await expect(cmd.draftAbandon(d, p.handle)).rejects.toMatchObject({ code: "send.prepared_tx_spent" });
    // A retry that lands is sent once: no second row, no second debit.
    fixture.world.send.broadcast = "ok";
    await expect(cmd.draftBroadcast(d, p.handle)).resolves.toEqual({ txid: p.summary.txid });
    expect((await cmd.balance(W))?.confirmed).toBe(3_019_698_287 - p.summary.totalDebit);
    expect((await cmd.historyPage(W, { categories: [], text: null, sort: "newestFirst", cursor: null, limit: 2 })).rows[1].txid).not.toBe(p.summary.txid);
    expect(await reserved()).toEqual([]);
  });

  it("keeps an unknown transaction's coins when its draft closes", async () => {
    fixture.world.send.broadcast = "unknown";
    const { d, p } = await prepared(pay(1_000_000));
    await expect(cmd.draftBroadcast(d, p.handle)).rejects.toMatchObject({ code: "send.broadcast_unknown" });
    const before = (await cmd.balance(W))?.confirmed;
    await cmd.closeDraft(d);
    await expect(cmd.draftBroadcast(d, p.handle)).rejects.toMatchObject({ code: "app.unknown_handle" });
    // Spent by the recorded transaction, not given back.
    expect((await cmd.balance(W))?.confirmed).toBe(before);
    const coins = await cmd.utxos(W, { includeLocked: true, fullyMixedOnly: false, minConfirmations: null });
    expect(coins.some((u) => p.summary.inputs.some((i) => i.outpoint.txid === u.outpoint.txid && i.outpoint.vout === u.outpoint.vout))).toBe(false);
  });

  it("counts change to a foreign address against the grant (U-03 D21)", async () => {
    const d = await draftFor(pay(50_000_000));
    await cmd.draftSetChange(d, { kind: "address", address: "yMpwVCKWHgSfCBbpB5fmhn7m2AFS2sMgtp" });
    const est = await cmd.draftEstimate(d);
    const short = await cmd.authorize({ kind: "spend", maxDuffs: est.totalSent + est.fee }, W, PASS);
    expect(await cmd.draftPrepare(d, short.id).catch((e) => e)).toMatchObject({ code: "send.grant_exceeded" });
    const full = await cmd.authorize({ kind: "spend", maxDuffs: est.totalSent + est.fee + (est.change ?? 0) }, W, PASS);
    const p = await cmd.draftPrepare(d, full.id);
    expect(p.summary).toMatchObject({ totalDebit: est.totalSent + est.fee + (est.change ?? 0), externalSent: est.totalSent + (est.change ?? 0) });
    expect(p.summary.outputs[1]).toMatchObject({ isChange: true, isMine: false });
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
    // dw-uri's format_request_uri: `user=` last, percent-encoded as QUrl does (DP3-03).
    expect(paymentUri(PAYEE, null, "A B", null, "alice")).toBe(`dash:${PAYEE}?label=A%20B&user=alice`);
    expect(paymentUri(PAYEE, null, null, null, "a(b)!")).toBe(`dash:${PAYEE}?user=a%28b%29%21`);
    expect(paymentUri(PAYEE, null, null, null, "")).toBe(`dash:${PAYEE}`);
    // As the engine (qurl::to_percent_encoding): only alphanumerics and `-._~` stay.
    expect(paymentUri(PAYEE, null, "Rent (May)!", null)).toBe(`dash:${PAYEE}?label=Rent%20%28May%29%21`);
    const qr = qrMatrix(paymentUri(PAYEE, 100_000_000, null, null));
    expect(qr.modules).toHaveLength(qr.size * qr.size);
    expect(() => qrMatrix("x".repeat(256))).toThrow();
  });
});
