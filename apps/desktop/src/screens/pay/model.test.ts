import { describe, expect, it } from "vitest";
import type { AppError } from "../../api/generated/AppError";
import type { CoinJoinStatus } from "../../api/generated/CoinJoinStatus";
import type { Utxo } from "../../api/generated/Utxo";
import type { LockStatus } from "../../api/generated/LockStatus";
import type { PreparedTxSummary } from "../../api/generated/PreparedTxSummary";
import type { PsbtAnalysis } from "../../api/generated/PsbtAnalysis";
import {
  acceptAndPayValue,
  addressProblem,
  authRefusal,
  availableLabel,
  broadcastFailure,
  changePolicy,
  coinSource,
  confirmActions,
  contactConfirmActions,
  contactDataText,
  failureText,
  paysWithUnmixed,
  PAY_CHECKING,
  PAY_INCOMPLETE,
  PAY_INCOMPLETE_CONTACT,
  reviewBlockedReason,
  confirmedOf,
  credentialPrompt,
  DEFAULT_ADVANCED,
  feeMode,
  keepsPromise,
  MIXED_INPUT_WARNING,
  MIXED_LEFT_OVER_WARNING,
  mixedSendNotes,
  unsignedKeepsPromise,
  outpointKey,
  outpointOf,
  recipientOf,
  mixedFeeText,
  sendErrorText,
  settleNotSent,
  multiRecipientPay,
  notSelectable,
  receivedAmounts,
  withSubtractFee,
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
const estimate = { fee: 226, sizeBytes: 226, inputCount: 1, change: 10, totalSent: 50_000_000, leftOver: 0 };
const confirmed = confirmedOf(recipient, estimate, { kind: "auto" });

const summary = (over: Partial<PreparedTxSummary> = {}): PreparedTxSummary => ({
  txid: "ab".repeat(32),
  fee: 226,
  feeRatePerKb: 1000,
  sizeBytes: 226,
  inputs: [{ outpoint: { txid: "cd".repeat(32), vout: 0 }, address: null, amount: 50_000_236 }],
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
    // Paid from the Platform balance (PayFromPlatform), not refused.
    expect(addressProblem({ kind: "platform" })).toBeNull();
    expect(addressProblem({ kind: "shielded" })).toMatch(/shielded address/);
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
    // 0.1.x-bal §1: the engine refuses a payment while a replay runs.
    // A replay ends by itself and needs no Repair; a stopped rescan does.
    const running = sendErrorText(err("rescan_in_progress"), "test");
    expect(running).toEqual({ text: "The wallet is catching up with the network. You can send once it has finished.", field: null });
    expect(running?.text).not.toMatch(/Repair|Rescan/);
    expect(sendErrorText(err("rescan_interrupted"), "test")).toEqual({
      text: "The last rescan was interrupted. Finish it in Repair ▸ Rescan before sending.",
      field: null,
    });
    expect(sendErrorText(err("wallet_not_found"), "test")).toBeNull();
    expect(sendErrorText(new Error("x"), "test")).toBeNull();
  });

  it("names what a normal payment can spend when the amount is over it, and where mixed coins are", () => {
    const over = err("send.amount_exceeds_balance", { kind: "duffs", needed: null, available: 65_276_809 });
    const plain = "The amount exceeds your balance.";
    const base = "The amount exceeds the 0.65276809 tDASH a normal payment can spend.";
    const where = "To spend fully mixed coins, choose Advanced: coins, fee and change ▸ Fully mixed funds only (CoinJoin).";
    // No mixing or mixed coins, or another source: the refusal keeps its plain text.
    const none = { mixed: false, mixing: false, stalled: false };
    const stalledAlone =
      "Denominations this wallet cannot mix stay apart; choose Advanced: coins, fee and change ▸ Selected coins to spend them, which can link your coins.";
    const stalledAfterMixed = "Denominations this wallet cannot mix use Selected coins there, which can link your coins.";
    const stalledAfterMixing =
      "Denominations this wallet cannot mix use Advanced: coins, fee and change ▸ Selected coins, which can link your coins.";
    const finishingAlone =
      "Coins still being mixed stay apart from normal payments; once mixing finishes them, they can be spent with Fully mixed funds only (CoinJoin).";
    const finishingBeside = "Coins still being mixed can be spent that way once mixing finishes.";
    expect(sendErrorText(over, "test", "DASH")?.text).toBe(plain);
    expect(sendErrorText(over, "test", "DASH", none)?.text).toBe(plain);
    expect(sendErrorText(over, "test", "DASH", { ...none, mixed: true })).toEqual({ text: `${base} ${where}`, field: "amount" });
    expect(sendErrorText(over, "test", "DASH", { ...none, mixed: true, mixing: true })?.text).toBe(`${base} ${where} ${finishingBeside}`);
    expect(sendErrorText(over, "test", "DASH", { ...none, mixing: true })?.text).toBe(`${base} ${finishingAlone}`);
    expect(sendErrorText(over, "test", "DASH", { ...none, stalled: true })?.text).toBe(`${base} ${stalledAlone}`);
    expect(sendErrorText(over, "test", "DASH", { ...none, mixed: true, stalled: true })?.text).toBe(`${base} ${where} ${stalledAfterMixed}`);
    expect(sendErrorText(over, "test", "DASH", { ...none, mixing: true, stalled: true })?.text).toBe(`${base} ${finishingAlone} ${stalledAfterMixing}`);
    // Without the engine's figure there is nothing to name.
    expect(sendErrorText(err("send.amount_exceeds_balance"), "test", "DASH", { ...none, mixed: true })?.text).toBe(plain);
  });

  it("labels Available with the coin source it counts", () => {
    expect(availableLabel("any")).toBe("Available");
    expect(availableLabel("mixed")).toBe("Available (fully mixed)");
    expect(availableLabel("selected")).toBe("Available (selected coins)");
  });

  it("says why a fully mixed payment's fee is absurd: no change, the rest is fee (MX-FEE)", () => {
    const mixed = { recipient, source: { kind: "fullyMixedOnly" } } as const;
    const absurd = err("send.absurd_fee", { kind: "fee", fee: 10_000_200, available: null });
    expect(mixedFeeText(absurd, mixed, "test")).toBe("Fully mixed coins are spent in whole denominations, so the 0.10000200 tDASH left over would go to fees.");
    expect(mixedFeeText(err("send.absurd_fee"), mixed, "test")).toBe("Fully mixed coins are spent in whole denominations, so what is left over would go to fees.");
    // Not for other coins, a payment already subtracting the fee, or another refusal.
    expect(mixedFeeText(absurd, { ...mixed, source: { kind: "any" } }, "test")).toBeNull();
    expect(mixedFeeText(absurd, { ...mixed, recipient: withSubtractFee(recipient) }, "test")).toBeNull();
    expect(mixedFeeText(err("send.dust_amount"), mixed, "test")).toBeNull();
    expect(withSubtractFee(recipient)).toEqual({ ...recipient, subtractFeeFromAmount: true });
  });

  it("is not sent only when the engine released a first attempt (E0-13 D8)", () => {
    const held = { txid: "ab".repeat(32), draft: 1, prepared: 2, amount: 5, fee: 1 };
    expect(broadcastFailure(err("send.broadcast_unknown"), "test", held, true)).toMatchObject({ kind: "unknown", retry: true, ...held });
    expect(broadcastFailure(err("send.broadcast_rejected", { kind: "reason", reason: "bad-txns" }), "test", held, true)).toEqual({
      kind: "failed",
      text: "The network rejected the transaction: bad-txns. The payment was not sent.",
    });
    for (const code of ["send.no_peers", "send.prepared_tx_spent", "send.cancelled", "app.unknown_handle"] as const)
      expect(broadcastFailure(err(code), "test", held, true).kind).toBe("failed");
    // Its record not written: refused before the hand-off, and a retry would be refused too.
    expect(broadcastFailure(err("storage"), "test", held, true)).toEqual({
      kind: "failed",
      text: "The payment could not be recorded in this wallet.",
    });
    expect(broadcastFailure(err("storage"), "test", held, false)).toMatchObject({ kind: "unknown", retry: true });
    // A code the renderer does not know may come after the bytes left: unknown, never abandoned.
    expect(broadcastFailure(err("internal"), "test", held, true)).toMatchObject({ kind: "unknown", retry: true });
  });

  it("reports a payment not sent even when its abandon is refused", async () => {
    const shown: string[] = [];
    await settleNotSent(() => Promise.reject(err("send.held_until_restart")), () => shown.push("refused"));
    let abandoned = false;
    await settleNotSent(
      async () => {
        abandoned = true;
      },
      () => shown.push("abandoned"),
    );
    await settleNotSent(null, () => shown.push("released"));
    expect(shown).toEqual(["refused", "abandoned", "released"]);
    expect(abandoned).toBe(true);
  });

  it("says a payment not sent whose coins stay reserved needs a restart, with no retry", () => {
    const held = { txid: "ab".repeat(32), draft: 1, prepared: 2, amount: 5, fee: 1 };
    const text =
      "The payment was not sent, but its coins stay reserved until you restart Dash Wallet. After the restart it's listed as pending in Activity; abandon it there to get the coins back.";
    expect(sendErrorText(err("send.held_until_restart"), "test")).toEqual({ text, field: null });
    for (const first of [true, false]) expect(broadcastFailure(err("send.held_until_restart"), "test", held, first)).toEqual({ kind: "failed", text });
  });

  it("never turns an unknown payment into not sent on a retry", () => {
    const held = { txid: "ab".repeat(32), draft: 1, prepared: 2, amount: 5, fee: 1 };
    expect(broadcastFailure(err("send.no_peers"), "test", held, false)).toMatchObject({ kind: "unknown", retry: true });
    expect(broadcastFailure(err("send.broadcast_rejected"), "test", held, false)).toMatchObject({ kind: "unknown", retry: true });
    // The handle went with the draft (a lock closed it): still unknown, nothing to retry.
    expect(broadcastFailure(err("app.unknown_handle"), "test", held, false)).toMatchObject({ kind: "unknown", retry: false });
  });
});

describe("a contact payment's refusals over the contact's data (H-09)", () => {
  const UNCHECKED = "Dash Wallet can't check this contact's payment details yet. Try again in a few minutes.";
  const MISMATCH = "Dash Wallet couldn't confirm this contact's payment details with the Dash network, so nothing was sent. Try again later.";

  it("says the contact's details can't be checked yet, or don't match", () => {
    expect(contactDataText(err("platform.context_unavailable"))).toBe(UNCHECKED);
    expect(contactDataText(err("platform.trust_mismatch"))).toBe(MISMATCH);
    expect(contactDataText(err("platform.unavailable"))).toBeNull();
    expect(contactDataText(new Error("x"))).toBeNull();
  });

  it("speaks of the contact only for a contact payment", () => {
    expect(failureText(err("platform.context_unavailable"), "test", "DASH", true)).toBe(UNCHECKED);
    expect(failureText(err("platform.trust_mismatch"), "test", "DASH", true)).toBe(MISMATCH);
    expect(failureText(err("platform.context_unavailable"), "test")).toBe("platform.context_unavailable");
    expect(failureText(err("platform.unavailable"), "test", "DASH", true)).toBe("Could not reach Dash Platform. Try again.");
  });
});

describe("the payment policy (E0-13 D11, U-03 D3)", () => {
  it("asks for the passphrase unless the signing window covers the amount", () => {
    const covered = { open: true, limitDuffs: 50_000_000, amount: 1_000 };
    expect(credentialPrompt(lock(), true, covered)).toBe("passphrase");
    // Not known yet: ask, never assume the weaker rule.
    expect(credentialPrompt(lock(), undefined, covered)).toBe("passphrase");
    expect(credentialPrompt(undefined, false, covered)).toBe("passphrase");
    expect(credentialPrompt(lock(), false, null)).toBe("passphrase");
    expect(credentialPrompt(lock(), false, { open: false, limitDuffs: 50_000_000, amount: 1_000 })).toBe("passphrase");
    expect(credentialPrompt(lock(), false, { open: true, limitDuffs: 50_000_000, amount: 50_000_001 })).toBe("passphrase");
    expect(credentialPrompt(lock(), false, covered)).toBe("none");
    expect(credentialPrompt(lock(), false, { open: true, limitDuffs: 50_000_000, amount: 50_000_000 })).toBe("none");
    expect(credentialPrompt(lock({ encrypted: false }), true, null)).toBe("none");
  });

  it("counts Accept and pay's quoted credits exactly, rounded up to whole duffs", () => {
    expect(acceptAndPayValue(1_000, { maxDuffs: 5, maxCredits: "1" })).toBe(1_006n);
    expect(acceptAndPayValue(0, { maxDuffs: 0, maxCredits: "150000000" })).toBe(150_000n);
    // Above 2^53 a Number rounds this to a whole number of duffs and drops the partial one.
    const credits = "9007199254741001";
    expect(Math.ceil(Number(credits) / 1000)).toBe(9_007_199_254_741);
    expect(acceptAndPayValue(0, { maxDuffs: 0, maxCredits: credits })).toBe(9_007_199_254_742n);
    // Past 2^53 duffs too: one duff over the window is still over.
    const limit = 2 ** 53;
    const over = { open: true, limitDuffs: limit, amount: acceptAndPayValue(limit, { maxDuffs: 0, maxCredits: "1" }) };
    expect(credentialPrompt(lock(), false, over)).toBe("passphrase");
    const at = { open: true, limitDuffs: limit, amount: acceptAndPayValue(limit, { maxDuffs: 0, maxCredits: "0" }) };
    expect(credentialPrompt(lock(), false, at)).toBe("none");
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
  it("is broken by a payment re-planned with other coins of the same total", () => {
    // Ten 0.100001 denominations make the same total as one 1.00001: Send must not quietly spend
    // ten mixed coins where the sheet showed one.
    expect(keepsPromise(confirmed, summary())).toBe(true);
    const input = summary().inputs[0];
    const ten = Array.from({ length: 10 }, (_, vout) => ({ ...input, outpoint: { ...input.outpoint, vout }, amount: 5_000_023 }));
    expect(keepsPromise(confirmed, summary({ inputs: [...ten, { ...input, amount: 6 }] }))).toBe(false);
  });

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

describe("what the confirm sheet offers (P15)", () => {
  it("sends, under a credential, from a wallet that signs", () => {
    expect(confirmActions(false, false)).toEqual({ send: true, credential: true, unsignedFirst: false });
  });

  it("makes only a PSBT when Advanced asked for one, even from a wallet that signs", () => {
    expect(confirmActions(true, false)).toEqual({ send: false, credential: false, unsignedFirst: true });
  });

  it("makes only a PSBT from a watch-only wallet, asked or not", () => {
    expect(confirmActions(false, true)).toEqual({ send: false, credential: false, unsignedFirst: true });
    expect(confirmActions(true, true)).toEqual({ send: false, credential: false, unsignedFirst: true });
  });

  it("holds Send while the wallet is not known", () => {
    expect(confirmActions(false, undefined)).toEqual({ send: false, credential: false, unsignedFirst: false });
  });
});

describe("what a contact's confirm sheet offers (#82)", () => {
  it("sends, under a credential, from a wallet that signs", () => {
    expect(contactConfirmActions(false)).toEqual({ send: true, credential: true, unsignedFirst: false });
  });

  it("never turns into the unsigned review: a contact payment cannot make a PSBT", () => {
    expect(contactConfirmActions(true)).toEqual({ send: false, credential: false, unsignedFirst: false });
  });

  it("holds Send while the wallet is not known", () => {
    expect(contactConfirmActions(undefined)).toEqual({ send: false, credential: false, unsignedFirst: false });
  });
});

describe("why a payment cannot be reviewed yet (P15)", () => {
  it("gives no reason once it can", () => {
    expect(reviewBlockedReason(true, ["stale"], true)).toBeNull();
  });

  it("names the first field problem the form shows, not an incomplete form", () => {
    const wrongNetwork = "This is a Mainnet address. This wallet is on Testnet.";
    expect(reviewBlockedReason(false, [wrongNetwork, null, undefined, "general"], true)).toBe(wrongNetwork);
    expect(reviewBlockedReason(false, [null, undefined, "Enter a valid amount."], false)).toBe("Enter a valid amount.");
  });

  it("falls back to incomplete or still being checked", () => {
    expect(reviewBlockedReason(false, [null, undefined, ""], false)).toBe(PAY_INCOMPLETE);
    expect(reviewBlockedReason(false, [null], true)).toBe(PAY_CHECKING);
  });

  it("asks the contact form only for an amount, and the address form for both", () => {
    expect(PAY_INCOMPLETE_CONTACT).toBe("Enter an amount first.");
    expect(PAY_INCOMPLETE).toBe("Enter an address and an amount first.");
    expect(reviewBlockedReason(false, [null], false, "contact")).toBe(PAY_INCOMPLETE_CONTACT);
    expect(reviewBlockedReason(false, [null], false, "address")).toBe(PAY_INCOMPLETE);
    expect(reviewBlockedReason(false, [null], true, "contact")).toBe(PAY_CHECKING);
  });
});

describe("mixedSendNotes (U-06b)", () => {
  const est = (inputCount: number, leftOver = 0) => ({ inputCount, leftOver });

  it("warns from 10 coins, as dash-qt does", () => {
    expect(MIXED_INPUT_WARNING).toBe(10);
    expect(mixedSendNotes("mixed", est(9), false)).toEqual({ inputCount: 9, manyCoins: false, leftOver: null, largeLeftOver: false });
    expect(mixedSendNotes("mixed", est(10), false)?.manyCoins).toBe(true);
    expect(mixedSendNotes("mixed", est(11), false)?.manyCoins).toBe(true);
    expect(mixedSendNotes("mixed", est(1), false)?.manyCoins).toBe(false);
  });

  it("names the left-over only when there is one", () => {
    expect(mixedSendNotes("mixed", est(2, 0), false)?.leftOver).toBeNull();
    expect(mixedSendNotes("mixed", est(2, 49_999_660), false)?.leftOver).toBe(49_999_660);
  });

  it("warns about a left-over from 0.01 DASH (#668)", () => {
    expect(MIXED_LEFT_OVER_WARNING).toBe(1_000_000);
    expect(mixedSendNotes("mixed", est(2, 999_999), false)?.largeLeftOver).toBe(false);
    expect(mixedSendNotes("mixed", est(2, 1_000_000), false)?.largeLeftOver).toBe(true);
    expect(mixedSendNotes("mixed", est(2), false)?.largeLeftOver).toBe(false);
  });

  it("leaves the left-over to the note while the 10% fee warning shows", () => {
    expect(mixedSendNotes("mixed", est(2, 1_000_000), true)).toEqual({ inputCount: 2, manyCoins: false, leftOver: 1_000_000, largeLeftOver: false });
  });

  it("says nothing for any other coin source", () => {
    expect(mixedSendNotes("any", est(11, 500), false)).toBeNull();
    expect(mixedSendNotes("selected", est(11, 500), false)).toBeNull();
  });
});

describe("paysWithUnmixed", () => {
  const status = (state: CoinJoinStatus["state"], fullyMixed: number) =>
    ({ state, balances: { fullyMixed }, amountAndRounds: { rounds: 4 } }) as unknown as CoinJoinStatus;
  const coin = (vout: number, denominated: boolean, rounds: number | null) =>
    ({ outpoint: { txid: "aa".repeat(32), vout }, coinjoinDenominated: denominated, coinjoinRounds: rounds }) as unknown as Utxo;
  const key = (vout: number) => `${"aa".repeat(32)}:${vout}`;
  const coins = [coin(0, false, null), coin(1, true, 4), coin(2, true, 7), coin(3, true, 2), coin(4, true, null)];
  const selected = (...vouts: number[]) => ({ ...DEFAULT_ADVANCED, coins: "selected" as const, selected: vouts.map(key) });

  it("says Any available funds is unmixed while mixing runs or mixed coins are held: it never spends a denomination", () => {
    expect(paysWithUnmixed(DEFAULT_ADVANCED, undefined, status("mixing", 0))).toBe(true);
    expect(paysWithUnmixed(DEFAULT_ADVANCED, undefined, status("idle", 1))).toBe(true);
  });

  it("says nothing in a wallet that does not mix, with an unknown status, or for a fully mixed payment", () => {
    expect(paysWithUnmixed(DEFAULT_ADVANCED, coins, status("idle", 0))).toBe(false);
    expect(paysWithUnmixed(DEFAULT_ADVANCED, coins, undefined)).toBe(false);
    expect(paysWithUnmixed({ ...DEFAULT_ADVANCED, coins: "mixed" }, coins, status("mixing", 5))).toBe(false);
  });

  it("says nothing for a selection of mixed coins, and says it once one selected coin is not mixed", () => {
    expect(paysWithUnmixed(selected(1, 2), coins, status("mixing", 5))).toBe(false);
    expect(paysWithUnmixed(selected(1, 0), coins, status("mixing", 5))).toBe(true);
    expect(paysWithUnmixed(selected(0), undefined, status("mixing", 5))).toBe(false);
  });

  it("does not call a partly mixed denomination, or one whose rounds are unknown, unmixed", () => {
    expect(paysWithUnmixed(selected(3), coins, status("mixing", 5))).toBe(false);
    expect(paysWithUnmixed(selected(4), coins, status("mixing", 5))).toBe(false);
    expect(paysWithUnmixed(selected(3, 4, 0), coins, status("mixing", 5))).toBe(true);
  });
});

describe("a payment to several recipients (QT-052)", () => {
  const BOB = "yBobBobBobBobBobBobBobBobBobBobBob";
  const CAROL = "yCarolCarolCarolCarolCarolCarolCa";
  const bob = recipientOf({ address: BOB, amount: 20_000_000, subtractFee: false, label: "Bob", message: "" });
  const carol = recipientOf({ address: CAROL, amount: 5_000_000, subtractFee: true, label: "", message: "" });
  // Alice 50 000 000, Bob 20 000 000, Carol 5 000 000 less the 300 fee.
  const est = { fee: 300, sizeBytes: 300, inputCount: 1, change: 10, totalSent: 74_999_700, leftOver: 0 };
  const many = confirmedOf(recipient, est, { kind: "auto" }, [bob, carol]);
  const outputs = [
    { address: PAYEE, amount: 50_000_000, isChange: false, label: null, isMine: false },
    { address: BOB, amount: 20_000_000, isChange: false, label: "Bob", isMine: false },
    { address: CAROL, amount: 4_999_700, isChange: false, label: null, isMine: false },
    { address: "yChange", amount: 10, isChange: true, label: null, isMine: true },
  ];
  const prepared = (over: Partial<PreparedTxSummary> = {}) =>
    summary({ fee: 300, outputs, totalSent: 74_999_700, totalDebit: 75_000_000, externalSent: 74_999_700, ...over });

  it("is off on mainnet only", () => {
    expect(multiRecipientPay({ kind: "mainnet" })).toBe(false);
    expect(multiRecipientPay({ kind: "testnet" })).toBe(true);
    expect(multiRecipientPay({ kind: "regtest" })).toBe(true);
  });

  it("shows what each receives: the one subtracting gets the rest of the total; two subtracting can't be shown", () => {
    expect(receivedAmounts([recipient, bob, carol], est.totalSent)).toEqual([50_000_000, 20_000_000, 4_999_700]);
    expect(receivedAmounts([recipient, bob], 70_000_000)).toEqual([50_000_000, 20_000_000]);
    expect(receivedAmounts([recipient, carol, { ...bob, subtractFeeFromAmount: true }], est.totalSent)).toBeNull();
    expect(many.total).toBe(75_000_000);
  });

  it("holds when each recipient is paid exactly its amount, in any output order", () => {
    expect(keepsPromise(many, prepared())).toBe(true);
    expect(keepsPromise(many, prepared({ outputs: [outputs[2], outputs[3], outputs[0], outputs[1]] }))).toBe(true);
  });

  it("fails on a recipient missing, paid twice, an amount moved between recipients, or an output the sheet didn't show", () => {
    expect(keepsPromise(many, prepared({ outputs: [outputs[0], outputs[1], outputs[3]] }))).toBe(false);
    // The same total, but Bob gets Alice's share and Alice Bob's.
    const swapped = [{ ...outputs[0], amount: 20_000_000 }, { ...outputs[1], amount: 50_000_000 }, outputs[2], outputs[3]];
    expect(keepsPromise(many, prepared({ outputs: swapped }))).toBe(false);
    // Carol's share paid to a stranger instead.
    expect(keepsPromise(many, prepared({ outputs: [outputs[0], outputs[1], { ...outputs[2], address: "yStranger" }, outputs[3]] }))).toBe(false);
    expect(keepsPromise(many, prepared({ outputs: [...outputs, { address: "yExtra", amount: 1, isChange: false, label: null, isMine: false }] }))).toBe(false);
    expect(keepsPromise(many, prepared({ fee: 301 }))).toBe(false);
    expect(keepsPromise(many, prepared({ totalDebit: 75_000_001 }))).toBe(false);
  });

  it("fails closed when two recipients subtract the fee, or one address is paid twice", () => {
    const two = confirmedOf(recipient, est, { kind: "auto" }, [{ ...bob, subtractFeeFromAmount: true }, carol]);
    expect(keepsPromise(two, prepared())).toBe(false);
    const dup = confirmedOf(recipient, est, { kind: "auto" }, [{ ...bob, address: PAYEE }, carol]);
    expect(keepsPromise(dup, prepared({ outputs: [outputs[0], { ...outputs[1], address: PAYEE }, outputs[2], outputs[3]] }))).toBe(false);
  });

  it("checks Create Unsigned's PSBT the same way", () => {
    const psbt = (out: PsbtAnalysis["outputs"]): PsbtAnalysis => ({
      outputs: out,
      fee: 300,
      total: 75_000_000,
      unsignedInputs: 1,
      status: "needsSignatures",
      signability: "canSign",
      externalSent: 74_999_700,
      unsignedTxid: "cd".repeat(32),
    });
    const ok = outputs.map(({ address, amount, isMine }) => ({ address, amount, isMine }));
    expect(unsignedKeepsPromise(many, psbt(ok))).toBe(true);
    expect(unsignedKeepsPromise(many, psbt([ok[0], { ...ok[1], amount: 50_000_000 }, { ...ok[2], amount: 19_999_700 }, ok[3]]))).toBe(false);
    expect(unsignedKeepsPromise(many, psbt(ok.slice(0, 3)))).toBe(false);
  });

  it("refuses a PSBT whose payments add up to more than the sheet showed as sent (one and several recipients)", () => {
    const psbt = (out: PsbtAnalysis["outputs"], fee = 226): PsbtAnalysis => ({
      outputs: out,
      fee,
      total: 0,
      unsignedInputs: 1,
      status: "needsSignatures",
      signability: "canSign",
      externalSent: 0,
      unsignedTxid: "cd".repeat(32),
    });
    // An estimate that says 0.9 is sent for a 1 DASH entry: a PSBT paying 1 DASH is not what the sheet showed.
    const one = confirmedOf({ ...recipient, amount: 100_000_000 }, { ...estimate, totalSent: 90_000_000 }, { kind: "auto" });
    expect(unsignedKeepsPromise(one, psbt([{ address: PAYEE, amount: 100_000_000, isMine: false }, { address: "yOwnChange", amount: 10, isMine: true }]))).toBe(false);
    const noSubtract = confirmedOf(recipient, { ...est, totalSent: 69_000_000 }, { kind: "auto" }, [bob]);
    const paid = [
      { address: PAYEE, amount: 50_000_000, isMine: false },
      { address: BOB, amount: 20_000_000, isMine: false },
      { address: "yOwnChange", amount: 10, isMine: true },
    ];
    expect(unsignedKeepsPromise(noSubtract, psbt(paid, 300))).toBe(false);
  });
});

describe("why a coin cannot be chosen", () => {
  const coin = (patch: Partial<Utxo> = {}): Utxo => ({
    outpoint: { txid: "ab".repeat(32), vout: 0 },
    address: "yAddressOne0000000000000000000001",
    amount: 100_000_000,
    confirmations: 10,
    blockHeight: 100,
    timestamp: 1_700_000_000,
    instantLocked: false,
    chainLocked: true,
    userLocked: false,
    reserved: false,
    label: null,
    isChange: false,
    isCoinbase: false,
    coinjoinDenominated: false,
    coinjoinRounds: null,
    spendable: true,
    checking: false,
    ...patch,
  });
  const CHECKING = "Checking whether it was spent elsewhere";

  it("says a coin under a block-spend check is being checked, before any other reason", () => {
    expect(notSelectable(coin({ checking: true, spendable: false }))).toBe(CHECKING);
    expect(notSelectable(coin({ checking: true, spendable: true }))).toBe(CHECKING);
    expect(notSelectable(coin({ checking: true, spendable: false, reserved: true }))).toBe(CHECKING);
    expect(notSelectable(coin({ checking: true, spendable: false, userLocked: true }))).toBe(CHECKING);
    expect(notSelectable(coin({ checking: true, spendable: false, isCoinbase: true }))).toBe(CHECKING);
    expect(notSelectable(coin({ checking: true, spendable: false, confirmations: 0 }))).toBe(CHECKING);
    expect(notSelectable(coin({ checking: true, spendable: false, reserved: true, userLocked: true, isCoinbase: true }))).toBe(CHECKING);
  });

  it("keeps every other coin's reason", () => {
    expect(notSelectable(coin())).toBeNull();
    expect(notSelectable(coin({ spendable: false, reserved: true }))).toBe("Reserved by a payment in progress");
    expect(notSelectable(coin({ spendable: false, userLocked: true }))).toBe("Locked");
    expect(notSelectable(coin({ spendable: false, isCoinbase: true }))).toBe("Immature");
    expect(notSelectable(coin({ spendable: false, confirmations: 0 }))).toBe("Not spendable");
  });
});
