import { describe, expect, it } from "vitest";
import type { AppError } from "../../api/generated/AppError";
import type { LockStatus } from "../../api/generated/LockStatus";
import type { PreparedTxSummary } from "../../api/generated/PreparedTxSummary";
import {
  addressProblem,
  authRefusal,
  broadcastFailure,
  coinSource,
  confirmedOf,
  credentialPrompt,
  DEFAULT_ADVANCED,
  feeMode,
  keepsPromise,
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
const confirmed = confirmedOf(recipient, { fee: 226, sizeBytes: 226, inputCount: 1, change: 10, totalSent: 50_000_000 });

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

  it("keeps the transaction only after broadcast_unknown (E0-13 D8)", () => {
    const held = { draft: 1, prepared: 2, amount: 5, fee: 1 };
    expect(broadcastFailure(err("send.broadcast_unknown"), "test", held)).toMatchObject({ kind: "unknown", ...held });
    expect(broadcastFailure(err("send.broadcast_rejected", { kind: "reason", reason: "bad-txns" }), "test", held)).toEqual({
      kind: "failed",
      text: "The network rejected the transaction: bad-txns. The payment was not sent.",
    });
    expect(broadcastFailure(err("send.no_peers"), "test", held).kind).toBe("failed");
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
      text: "The passphrase you entered was incorrect. 3 failed attempts. Try again in 30 s.",
    });
    expect(authRefusal(err("vault.no_vault")).kind).toBe("other");
  });
});

describe("the confirm sheet's promise (U-03 D2)", () => {
  it("caps the grant at the amount plus the fee", () => {
    expect(confirmed.total).toBe(50_000_226);
  });

  it("holds when the signed transaction pays what the sheet showed", () => {
    expect(keepsPromise(confirmed, summary())).toBe(true);
  });

  it("fails when the payee, the amount or the debit differ", () => {
    const outputs = summary().outputs;
    expect(keepsPromise(confirmed, summary({ outputs: [{ ...outputs[0], address: "yOther" }, outputs[1]] }))).toBe(false);
    expect(keepsPromise(confirmed, summary({ outputs: [{ ...outputs[0], amount: 49_000_000 }, outputs[1]], totalSent: 49_000_000 }))).toBe(false);
    expect(keepsPromise(confirmed, summary({ totalDebit: 50_000_227 }))).toBe(false);
    // A change output to the payee's address does not count as paying it.
    expect(keepsPromise(confirmed, summary({ outputs: [{ ...outputs[0], amount: 25_000_000 }, { ...outputs[1], address: PAYEE, amount: 25_000_000 }] }))).toBe(false);
  });
});
