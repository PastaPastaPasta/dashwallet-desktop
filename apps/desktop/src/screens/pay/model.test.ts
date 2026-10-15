import { describe, expect, it } from "vitest";
import type { AppError } from "../../api/generated/AppError";
import type { LockStatus } from "../../api/generated/LockStatus";
import type { PreparedTxSummary } from "../../api/generated/PreparedTxSummary";
import type { PsbtAnalysis } from "../../api/generated/PsbtAnalysis";
import {
  addressProblem,
  authRefusal,
  broadcastFailure,
  changePolicy,
  coinSource,
  confirmedOf,
  credentialPrompt,
  DEFAULT_ADVANCED,
  feeMode,
  keepsPromise,
  unsignedKeepsPromise,
  outpointKey,
  outpointOf,
  recipientOf,
  sendErrorText,
} from "./model";

const err = (code: AppError["code"], data: AppError["data"] = null): AppError => ({ code, message: code, data });
const lock = (over: Partial<LockStatus> = {}): LockStatus => ({
  state: "unlocked",
  encrypted: true,
  quickUnlockEnrolled: false,
  failedAttempts: 0,
  retryAfterSecs: null,
  walletsWithSecrets: [],
  ...over,
});
const PAYEE = "yXdjeVZqmWp4ZcVJ2Hh8hcFhUzFE8wD1Cd";
const recipient = recipientOf({ address: PAYEE, amount: 50_000_000, subtractFee: false, label: "  Alice ", message: " " });
const estimate = { fee: 226, sizeBytes: 226, inputCount: 1, change: 10, totalSent: 50_000_000 };
const confirmed = confirmedOf(recipient, estimate, { kind: "auto" });

const summary = (over: Partial<PreparedTxSummary> = {}): PreparedTxSummary => ({
  txid: "ab".repeat(32),
  fee: 226,
  feeRatePerKb: 1000,
  sizeBytes: 226,
  inputs: [],
  outputs: [
    { address: PAYEE, amount: 50_000_000, isChange: false, label: null, isMine: false },
    { address: "yChange", amount: 10, isChange: true, label: null, isMine: true },
  ],
  totalSent: 50_000_000,
  totalDebit: 50_000_226,
  externalSent: 50_000_000,
  ...over,
});

describe("the form's draft", () => {
  it("trims label and message to null", () => {
    expect(recipient).toEqual({ address: PAYEE, amount: 50_000_000, subtractFeeFromAmount: false, label: "Alice", message: null });
  });

  it("maps the Advanced panel to the draft's source and fee", () => {
    expect(coinSource(DEFAULT_ADVANCED)).toEqual({ kind: "any" });
    expect(coinSource({ ...DEFAULT_ADVANCED, coins: "mixed" })).toEqual({ kind: "fullyMixedOnly" });
    const key = outpointKey({ txid: "ab".repeat(32), vout: 3 });
    expect(outpointOf(key)).toEqual({ txid: "ab".repeat(32), vout: 3 });
    expect(coinSource({ ...DEFAULT_ADVANCED, coins: "selected", selected: [key] })).toEqual({ kind: "outpoints", outpoints: [outpointOf(key)] });
    expect(feeMode(DEFAULT_ADVANCED)).toEqual({ kind: "recommended", targetBlocks: 6 });
    // A custom rate that does not parse yet holds the payment.
    expect(feeMode({ ...DEFAULT_ADVANCED, fee: { kind: "custom", duffsPerKb: null } })).toBeNull();
    expect(feeMode({ ...DEFAULT_ADVANCED, fee: { kind: "custom", duffsPerKb: 5000 } })).toEqual({ kind: "perKb", duffsPerKb: 5000 });
  });

  it("holds the payment while the custom change address is empty", () => {
    expect(changePolicy(DEFAULT_ADVANCED)).toEqual({ kind: "auto" });
    expect(changePolicy({ ...DEFAULT_ADVANCED, changeAddress: "  " })).toBeNull();
    expect(changePolicy({ ...DEFAULT_ADVANCED, changeAddress: " yChange " })).toEqual({ kind: "address", address: "yChange" });
  });

  it("explains addresses it cannot pay", () => {
    expect(addressProblem({ kind: "core", scriptHash: false })).toBeNull();
    expect(addressProblem({ kind: "platform" })).toMatch(/Platform address/);
    expect(addressProblem({ kind: "invalid", problem: { kind: "invalidBase58Prefix" } })).toBe("This address is not valid on this network.");
    expect(addressProblem({ kind: "invalid", problem: { kind: "invalidBase58Length" } })).toMatch(/not valid/);
  });
});

describe("the engine's refusals", () => {
  it("puts dash-qt's text on the field it is about", () => {
    expect(sendErrorText(err("send.dust_amount"), "test")).toMatchObject({ field: "amount" });
    expect(sendErrorText(err("send.invalid_address", { kind: "recipient", index: 0 }), "test")).toMatchObject({ field: "address" });
    expect(sendErrorText(err("send.amount_with_fee_exceeds_balance", { kind: "fee", fee: 374, available: 1 }), "test")?.text).toBe(
      "The total exceeds your balance when the 0.00000374 tDASH transaction fee is included.",
    );
    expect(sendErrorText(err("send.absurd_fee"), "test")).toEqual({ text: "The fee is absurdly high.", field: null });
    expect(sendErrorText(err("wallet_not_found"), "test")).toBeNull();
    expect(sendErrorText(new Error("x"), "test")).toBeNull();
  });

  it("is not sent only when the engine released a first attempt (E0-13 D8)", () => {
    const held = { draft: 1, prepared: 2, amount: 5, fee: 1 };
    expect(broadcastFailure(err("send.broadcast_unknown"), "test", held, true)).toMatchObject({ kind: "unknown", retry: true, ...held });
    expect(broadcastFailure(err("send.broadcast_rejected", { kind: "reason", reason: "bad-txns" }), "test", held, true)).toEqual({
      kind: "failed",
      text: "The network rejected the transaction: bad-txns. The payment was not sent.",
    });
    for (const code of ["send.no_peers", "send.prepared_tx_spent", "send.cancelled", "app.unknown_handle"] as const)
      expect(broadcastFailure(err(code), "test", held, true).kind).toBe("failed");
    // A code the renderer does not know may come after the bytes left: unknown, never abandoned.
    expect(broadcastFailure(err("internal"), "test", held, true)).toMatchObject({ kind: "unknown", retry: true });
  });

  it("never turns an unknown payment into not sent on a retry", () => {
    const held = { draft: 1, prepared: 2, amount: 5, fee: 1 };
    expect(broadcastFailure(err("send.no_peers"), "test", held, false)).toMatchObject({ kind: "unknown", retry: true });
    expect(broadcastFailure(err("send.broadcast_rejected"), "test", held, false)).toMatchObject({ kind: "unknown", retry: true });
    // The handle went with the draft (a lock closed it): still unknown, nothing to retry.
    expect(broadcastFailure(err("app.unknown_handle"), "test", held, false)).toMatchObject({ kind: "unknown", retry: false });
  });
});

describe("the payment policy (E0-13 D11, U-03 D3)", () => {
  it("asks for the passphrase unless the vault is unencrypted or the setting is off", () => {
    expect(credentialPrompt(lock(), true)).toBe("passphrase");
    // Not known yet: ask, never assume the weaker rule.
    expect(credentialPrompt(lock(), undefined)).toBe("passphrase");
    expect(credentialPrompt(undefined, false)).toBe("passphrase");
    expect(credentialPrompt(lock(), false)).toBe("none");
    expect(credentialPrompt(lock({ encrypted: false }), true)).toBe("none");
  });

  it("reads authorize's refusals", () => {
    expect(authRefusal(err("vault.credential_required"))).toEqual({ kind: "needsPassphrase" });
    expect(authRefusal(err("vault.wrong_passphrase", { kind: "wrongPassphrase", failedAttempts: 3, retryAfterSecs: 30 }))).toEqual({
      kind: "wrongPassphrase",
      text: "The passphrase you entered was incorrect. 3 failed attempts.",
      waitSecs: 30,
    });
    expect(authRefusal(err("vault.throttled", { kind: "retryAfter", secs: 90 }))).toEqual({ kind: "wrongPassphrase", text: "Too many attempts.", waitSecs: 90 });
    expect(authRefusal(err("vault.wrong_passphrase", { kind: "wrongPassphrase", failedAttempts: 1, retryAfterSecs: null }))).toMatchObject({ waitSecs: 0 });
    expect(authRefusal(err("vault.no_vault")).kind).toBe("other");
  });
});

describe("the confirm sheet's promise (U-03 D2)", () => {
  it("caps the grant at the amount plus the fee", () => {
    expect(confirmed.total).toBe(50_000_226);
  });

  it("counts the change in the cap when it goes to a custom address (D21)", () => {
    const custom = confirmedOf(recipient, estimate, { kind: "address", address: "yForeign" });
    expect(custom).toMatchObject({ changeAddress: "yForeign", total: 50_000_236 });
    const outputs = summary().outputs;
    const foreign = summary({ outputs: [outputs[0], { ...outputs[1], address: "yForeign", isMine: false }], totalDebit: 50_000_236 });
    expect(keepsPromise(custom, foreign)).toBe(true);
    expect(keepsPromise(confirmed, foreign)).toBe(false);
    // An output the sheet never showed, paid for by smaller change, within the total.
    const extra = { address: "yForeign2", amount: 5, isChange: false, label: null, isMine: false };
    expect(keepsPromise(custom, summary({ outputs: [outputs[0], extra, { ...outputs[1], address: "yForeign", amount: 5, isMine: false }], totalDebit: 50_000_236 }))).toBe(false);
    // Change somewhere the sheet did not show.
    expect(keepsPromise(custom, summary({ outputs: [outputs[0], { ...outputs[1], address: "yElse", isMine: false }], totalDebit: 50_000_236 }))).toBe(false);
  });

  it("holds when the signed transaction pays what the sheet showed", () => {
    expect(keepsPromise(confirmed, summary())).toBe(true);
  });

  it("refuses a fee/change trade that stays within the D21 cap (D23)", () => {
    const custom = confirmedOf(recipient, estimate, { kind: "address", address: "yForeign" });
    const outputs = summary().outputs;
    // 4 duffs moved from the custom change to the fee: same debit, same cap, a different payment.
    const traded = summary({ fee: 230, outputs: [outputs[0], { ...outputs[1], address: "yForeign", amount: 6, isMine: false }], totalDebit: 50_000_236 });
    expect(traded.totalDebit).toBeLessThanOrEqual(custom.total);
    expect(keepsPromise(custom, traded)).toBe(false);
  });

  it("refuses a different fee even with the total unchanged (D23)", () => {
    expect(keepsPromise(confirmed, summary({ fee: 230 }))).toBe(false);
    // And different wallet-owned change, as when the coins changed.
    const outputs = summary().outputs;
    expect(keepsPromise(confirmed, summary({ outputs: [outputs[0], { ...outputs[1], amount: 11 }] }))).toBe(false);
  });

  it("passes an exact match, also when the payment or the custom change stays in the wallet", () => {
    const outputs = summary().outputs;
    const custom = confirmedOf(recipient, estimate, { kind: "address", address: "yOwn" });
    expect(keepsPromise(custom, summary({ outputs: [outputs[0], { ...outputs[1], address: "yOwn", isMine: true }], totalDebit: 50_000_226 }))).toBe(true);
    // Paying one of the wallet's own addresses: only the fee leaves it.
    expect(keepsPromise(confirmed, summary({ outputs: [{ ...outputs[0], isMine: true }, outputs[1]], totalDebit: 226, externalSent: 0 }))).toBe(true);
    expect(keepsPromise(confirmed, summary({ outputs: [{ ...outputs[0], isMine: true }, outputs[1]], totalDebit: 50_000_226 }))).toBe(false);
  });

  it("fails when the payee, the amount or the debit differ", () => {
    const outputs = summary().outputs;
    expect(keepsPromise(confirmed, summary({ outputs: [{ ...outputs[0], address: "yOther" }, outputs[1]] }))).toBe(false);
    expect(keepsPromise(confirmed, summary({ outputs: [{ ...outputs[0], amount: 49_000_000 }, outputs[1]], totalSent: 49_000_000 }))).toBe(false);
    expect(keepsPromise(confirmed, summary({ totalDebit: 50_000_227 }))).toBe(false);
    expect(keepsPromise(confirmed, summary({ totalDebit: 50_000_225 }))).toBe(false);
    // A change output to the payee's address does not count as paying it.
    expect(keepsPromise(confirmed, summary({ outputs: [{ ...outputs[0], amount: 25_000_000 }, { ...outputs[1], address: PAYEE, amount: 25_000_000 }] }))).toBe(false);
  });
});

describe("Create Unsigned keeps the sheet's promise (U-05 D37)", () => {
  const unsigned = (over: Partial<PsbtAnalysis> = {}): PsbtAnalysis => ({
    outputs: [
      { address: PAYEE, amount: 50_000_000, isMine: false },
      { address: "yOwnChange", amount: 10, isMine: true },
    ],
    fee: 226,
    total: 50_000_226,
    unsignedInputs: 1,
    status: "needsSignatures",
    signability: "canSign",
    externalSent: 50_000_000,
    unsignedTxid: "cd".repeat(32),
    ...over,
  });
  const [pay, change] = unsigned().outputs;

  it("holds for the PSBT of the plan the sheet showed", () => {
    expect(unsignedKeepsPromise(confirmed, unsigned())).toBe(true);
    const custom = confirmedOf(recipient, estimate, { kind: "address", address: "yForeign" });
    expect(unsignedKeepsPromise(custom, unsigned({ outputs: [pay, { address: "yForeign", amount: 10, isMine: false }] }))).toBe(true);
    // Change somewhere other than the custom address the sheet showed.
    expect(unsignedKeepsPromise(custom, unsigned())).toBe(false);
    // No change: the payment alone.
    const whole = confirmedOf(recipient, { ...estimate, change: null }, { kind: "auto" });
    expect(unsignedKeepsPromise(whole, unsigned({ outputs: [pay] }))).toBe(true);
    expect(unsignedKeepsPromise(whole, unsigned())).toBe(false);
    // Subtract fee: the recipient gets the estimate's amount after the fee.
    const net = recipientOf({ address: PAYEE, amount: 50_000_000, subtractFee: true, label: "", message: "" });
    const lessFee = confirmedOf(net, { ...estimate, totalSent: 49_999_774 }, { kind: "auto" });
    expect(unsignedKeepsPromise(lessFee, unsigned({ outputs: [{ ...pay, amount: 49_999_774 }, change] }))).toBe(true);
    expect(unsignedKeepsPromise(lessFee, unsigned())).toBe(false);
    // Change sent back to the recipient's own address, as the sheet showed.
    const back = confirmedOf(recipient, estimate, { kind: "address", address: PAYEE });
    expect(unsignedKeepsPromise(back, unsigned({ outputs: [{ ...pay, amount: 10 }, pay] }))).toBe(true);
    expect(unsignedKeepsPromise(back, unsigned({ outputs: [pay, pay] }))).toBe(false);
  });

  it("refuses a plan that changed after the estimate: fee, change, amount, coins or outputs", () => {
    expect(unsignedKeepsPromise(confirmed, unsigned({ fee: 400, outputs: [pay, { ...change, amount: 1 }] }))).toBe(false);
    expect(unsignedKeepsPromise(confirmed, unsigned({ outputs: [pay] }))).toBe(false);
    // A subtract-fee payment that now pays less.
    expect(unsignedKeepsPromise(confirmed, unsigned({ outputs: [{ ...pay, amount: 49_999_000 }, change] }))).toBe(false);
    // Another coin (the fee estimate rounded to the same).
    expect(unsignedKeepsPromise(confirmed, unsigned({ unsignedInputs: 2 }))).toBe(false);
    expect(unsignedKeepsPromise(confirmed, unsigned({ outputs: [pay, change, { address: "yOther", amount: 5, isMine: false }] }))).toBe(false);
    expect(unsignedKeepsPromise(confirmed, unsigned({ outputs: [pay, { ...change, isMine: false }] }))).toBe(false);
    expect(unsignedKeepsPromise(confirmed, unsigned({ fee: null }))).toBe(false);
  });
});
