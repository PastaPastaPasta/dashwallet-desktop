// Pay ▸ To an address, the logic without React (U-03): which credential the confirm sheet
// asks for, what the engine's send refusals say and where, what the confirm sheet promised and
// whether the signed transaction keeps that promise, and how a broadcast ended. A payment to a
// DashPay contact (DP3-06) shares all of it; its own parts are in contact.ts.

import { hasCode, isAppError, retryAfterSecs } from "../../api/backend";
import type { AddressClass } from "../../api/generated/AddressClass";
import type { AppError } from "../../api/generated/AppError";
import type { ChangePolicy } from "../../api/generated/ChangePolicy";
import type { ContactRecipient } from "../../api/generated/ContactRecipient";
import type { CoinSource } from "../../api/generated/CoinSource";
import type { FeeMode } from "../../api/generated/FeeMode";
import type { LockStatus } from "../../api/generated/LockStatus";
import type { PreparedTxSummary } from "../../api/generated/PreparedTxSummary";
import type { PsbtAnalysis } from "../../api/generated/PsbtAnalysis";
import type { Recipient } from "../../api/generated/Recipient";
import type { TxEstimate } from "../../api/generated/TxEstimate";
import type { OutPoint } from "../../api/generated/OutPoint";
import type { Utxo } from "../../api/generated/Utxo";
import { BROKEN } from "../../contacts/pane";
import { formatWithUnit, type Chain, type Unit } from "../../format/amount";

/** The coins a payment may use (coin control, U-03 D7). */
export type CoinChoice = "any" | "mixed" | "selected";

/** The Advanced panel's choices; the defaults are the engine's draft defaults. */
export interface Advanced {
  coins: CoinChoice;
  /** `txid:vout` of the selected coins. */
  selected: string[];
  fee: { kind: "recommended"; targetBlocks: number } | { kind: "custom"; duffsPerKb: number | null };
  /** `null`: automatic change. */
  changeAddress: string | null;
}

/** dash-qt's default confirmation target (6 blocks), the engine draft's too. */
export const DEFAULT_TARGET_BLOCKS = 6;

export const DEFAULT_ADVANCED: Advanced = {
  coins: "any",
  selected: [],
  fee: { kind: "recommended", targetBlocks: DEFAULT_TARGET_BLOCKS },
  changeAddress: null,
};

export const outpointKey = (o: OutPoint) => `${o.txid}:${o.vout}`;

export function outpointOf(key: string): OutPoint {
  const i = key.lastIndexOf(":");
  return { txid: key.slice(0, i), vout: Number(key.slice(i + 1)) };
}

/** Why a coin cannot be chosen, or `null` when it can (Pay's coin control and Tools ▸ Coin
 * Selection, U-05 D19). */
export function notSelectable(u: Utxo): string | null {
  if (u.spendable) return null;
  if (u.reserved) return "Reserved by a payment in progress";
  if (u.userLocked) return "Locked";
  if (u.isCoinbase) return "Immature";
  return "Not spendable";
}

export function coinSource(a: Advanced): CoinSource {
  switch (a.coins) {
    case "any":
      return { kind: "any" };
    case "mixed":
      return { kind: "fullyMixedOnly" };
    case "selected":
      return { kind: "outpoints", outpoints: a.selected.map(outpointOf) };
  }
}

/** The draft's fee mode; `null` while a custom rate is not a number yet. */
export function feeMode(a: Advanced): FeeMode | null {
  if (a.fee.kind === "recommended") return { kind: "recommended", targetBlocks: a.fee.targetBlocks };
  return a.fee.duffsPerKb === null ? null : { kind: "perKb", duffsPerKb: a.fee.duffsPerKb };
}

/** The draft's change: automatic, or the custom address (trimmed); `null` holds the payment
 * while the custom address is empty. */
export function changePolicy(a: Advanced): ChangePolicy | null {
  if (a.changeAddress === null) return { kind: "auto" };
  const address = a.changeAddress.trim();
  return address === "" ? null : { kind: "address", address };
}

/** What the form holds once its fields parse. */
export interface PayInput {
  address: string;
  /** Duffs. */
  amount: number;
  subtractFee: boolean;
  label: string;
  message: string;
}

/** What a payment pays: an address, or a DashPay contact whose address the engine reserves at
 * prepare (DP3-06 D8). */
export type PayRecipient = Recipient | ContactRecipient;

export const isContact = (r: PayRecipient): r is ContactRecipient => "contact" in r;

export const subtractsFee = (r: PayRecipient) => (isContact(r) ? r.subtractFee : r.subtractFeeFromAmount);

export function recipientOf(input: PayInput): Recipient {
  return {
    address: input.address,
    amount: input.amount,
    subtractFeeFromAmount: input.subtractFee,
    label: input.label.trim() || null,
    message: input.message.trim() || null,
  };
}

// ---------------------------------------------------------------- field checks

/** Why an address cannot be paid on this network (`classifyAddress`), or `null` when it can. */
export function addressProblem(c: AddressClass): string | null {
  switch (c.kind) {
    case "core":
      return null;
    case "platform":
      return "This is a Dash Platform address. Pay it from Platform, not from this wallet.";
    case "shielded":
      return "This is a shielded address. This wallet cannot pay shielded addresses yet.";
    case "invalid":
      return c.problem.kind === "invalidBase58Prefix" ? "This address is not valid on this network." : "The recipient address is not valid. Please recheck.";
  }
}

// ---------------------------------------------------------------- the engine's refusals

export type Field = "address" | "amount" | null;

/** dash-qt's send-dialog text for a refusal (`SendCoinsDialog::processSendCoinsReturn`), and the
 * field it belongs to; `null` when the error is not a send refusal. */
export function sendErrorText(e: unknown, chain: Chain, unit: Unit = "DASH"): { text: string; field: Field } | null {
  if (!isAppError(e)) return null;
  const amount = (duffs: number) => formatWithUnit(unit, duffs, false, "standard", chain);
  switch (e.code) {
    case "send.no_recipients":
      return { text: "Enter an address and an amount.", field: null };
    case "send.invalid_address":
    case "invalid_address":
      return { text: "The recipient address is not valid. Please recheck.", field: "address" };
    case "send.platform_address":
      return { text: "This is a Dash Platform address. Pay it from Platform, not from this wallet.", field: "address" };
    case "send.duplicate_address":
      return { text: "Duplicate address found: addresses should only be used once each.", field: "address" };
    case "send.invalid_amount":
      return { text: "The amount to pay must be larger than 0.", field: "amount" };
    case "send.dust_amount":
      return { text: "The amount is too small to send: it is below the dust threshold.", field: "amount" };
    case "send.amount_too_small_after_fee":
      return { text: "The amount is too small to pay its share of the fee.", field: "amount" };
    case "send.amount_exceeds_balance":
      return { text: "The amount exceeds your balance.", field: "amount" };
    case "send.amount_with_fee_exceeds_balance": {
      const fee = e.data?.kind === "fee" ? e.data.fee : null;
      return {
        text: fee === null ? "The total exceeds your balance when the transaction fee is included." : `The total exceeds your balance when the ${amount(fee)} transaction fee is included.`,
        field: "amount",
      };
    }
    case "send.insufficient_mixed_funds": {
      const available = e.data?.kind === "duffs" ? e.data.available : null;
      return { text: available === null ? "Not enough fully mixed funds." : `Not enough fully mixed funds: ${amount(available)} available.`, field: "amount" };
    }
    case "send.outpoint_unavailable":
      return { text: "A selected coin is spent, locked or no longer in the wallet. Choose the coins again.", field: null };
    case "send.absurd_fee": {
      const fee = e.data?.kind === "fee" ? e.data.fee : null;
      return { text: fee === null ? "The fee is absurdly high." : `A fee of ${amount(fee)} is considered an absurdly high fee.`, field: null };
    }
    case "send.tx_too_large":
      return { text: "The transaction is too large. Send a smaller amount, or choose fewer coins.", field: null };
    case "send.invalid_change_address":
      return { text: "The custom change address is not valid.", field: null };
    case "send.watch_only":
      return { text: "This is a watch-only wallet: it cannot sign payments.", field: null };
    case "send.vault_locked":
    case "vault.locked":
      return { text: "The wallet is locked. Unlock it to send.", field: null };
    case "send.grant_invalid":
    case "vault.grant_invalid":
      return { text: "The authorization expired. Send again.", field: null };
    case "send.grant_exceeded":
      return { text: "The payment changed. Review it again.", field: null };
    case "send.prepared_tx_spent":
      return { text: "Its coins were spent meanwhile. Review the payment again.", field: null };
    case "send.no_peers":
      return { text: "No connection to the Dash network. The payment was not sent.", field: null };
    case "send.broadcast_rejected":
      return {
        text: `The network rejected the transaction${e.data?.kind === "reason" ? `: ${e.data.reason}` : ""}. The payment was not sent.`,
        field: null,
      };
    case "send.broadcast_unknown":
      return { text: "The payment may have been sent: the network did not answer in time.", field: null };
    case "send.cancelled":
      return { text: "The payment was cancelled before anything was sent.", field: null };
    // A payment to a contact (DP3-06 D13).
    case "contact.payment_locked":
      return { text: "Another payment to this contact is still being sent or may have been sent. Pay again once it is settled.", field: null };
    case "contact.channel_broken":
      return { text: BROKEN, field: null };
    case "contact.ineligible":
      return {
        text: e.data?.kind === "ineligible" && e.data.reason === "pendingIncoming" ? "Accept the contact request first, or use Accept and pay." : "This contact cannot be paid now.",
        field: null,
      };
    case "contact.self":
      return { text: "This is you.", field: null };
    case "identity.not_found":
      return { text: "Your DashPay identity is not available. Try again once DashPay has synced.", field: null };
    case "platform.unavailable":
    case "platform.timeout":
      return { text: "Could not reach Dash Platform. Try again.", field: null };
    case "platform.lease_revoked":
      return { text: "The wallet locked during the payment.", field: null };
    case "platform.lease_expired":
    case "platform.grant_invalid":
      return { text: "The authorization expired. Send again.", field: null };
    case "platform.needs_grant":
    case "platform.signer_unavailable":
      return { text: "Unlock the wallet and try again.", field: null };
    case "platform.grant_exceeded":
      return { text: "The payment changed. Review it again.", field: null };
    case "storage":
      return { text: "The payment could not be recorded in this wallet.", field: null };
    default:
      return null;
  }
}

// ---------------------------------------------------------------- the payment policy (E0-13 D11)

/** The credential the confirm sheet asks for: the passphrase when the vault is encrypted and
 * "Require authentication for every payment" is on or unknown (U-03 D3); otherwise the
 * unlocked vault's own (`unencrypted`). The engine enforces the rule either way. */
export function credentialPrompt(lock: LockStatus | undefined, paymentAuth: boolean | undefined): "passphrase" | "none" {
  if (lock === undefined) return "passphrase";
  return lock.encrypted && paymentAuth !== false ? "passphrase" : "none";
}

/** What an `authorize` refusal means for the confirm sheet. */
export type AuthRefusal =
  | { kind: "needsPassphrase" }
  /** `waitSecs`: the vault refuses every attempt for that long (0: none). */
  | { kind: "wrongPassphrase"; text: string; waitSecs: number }
  | { kind: "other" };

export function authRefusal(e: AppError): AuthRefusal {
  if (e.code === "vault.credential_required") return { kind: "needsPassphrase" };
  if (e.code === "vault.wrong_passphrase" || e.code === "vault.throttled") {
    const wait = retryAfterSecs(e);
    const attempts = e.data?.kind === "wrongPassphrase" ? e.data.failedAttempts : null;
    const base = e.code === "vault.throttled" ? "Too many attempts." : "The passphrase you entered was incorrect.";
    const count = attempts !== null && attempts > 1 ? ` ${attempts} failed attempts.` : "";
    return { kind: "wrongPassphrase", text: `${base}${count}`, waitSecs: wait };
  }
  return { kind: "other" };
}

// ---------------------------------------------------------------- confirm and check

/** What the confirm sheet showed and the user agreed to. */
export interface Confirmed {
  recipient: PayRecipient;
  estimate: TxEstimate;
  /** The custom change address, if any. */
  changeAddress: string | null;
  /** What leaves the wallet at most: the recipients' amount plus the fee, and the change when it
   * goes to a custom address (U-03 D21: the engine counts it if the wallet does not own the
   * address, and the renderer cannot tell). The `spend` grant's cap. */
  total: number;
}

export function confirmedOf(recipient: PayRecipient, estimate: TxEstimate, change: ChangePolicy): Confirmed {
  const changeAddress = change.kind === "address" ? change.address : null;
  const total = estimate.totalSent + estimate.fee + (changeAddress === null ? 0 : (estimate.change ?? 0));
  return { recipient, estimate, changeAddress, total };
}

/** The signed transaction keeps the sheet's promise (U-03 D2, D23): every number the sheet showed
 * is the signed transaction's. Every output is the payment to the address the sheet showed or
 * change where it said (the wallet's own, or the custom address); the payment, the fee and the
 * change are exactly the confirmed ones; and the wallet's debit is exactly the confirmed total,
 * less what of it comes back to the wallet (a payment to its own address, a custom change address
 * it owns). Anything else, even within the grant's cap, is a different payment. A contact's
 * address is the one the engine reserved at prepare (DP3-06 D9): its payment is the one output
 * that is neither change nor the wallet's. */
/** The engine's stand-in for a contact's address in an estimate (`send/contact.rs` `placeholder`:
 * P2PKH of 20 zero bytes, mainnet and testnet), which a signed payment must never pay. */
const CONTACT_PLACEHOLDERS = ["XagqqFetxiDb9wbartKDrXgnqLah6SqX2S", "yLKSrCjLQFsfVgX8RjdctZ797d54atPjnV"];

export function keepsPromise(c: Confirmed, s: PreparedTxSummary): boolean {
  const { estimate, recipient: r } = c;
  const paysPayee = (o: PreparedTxSummary["outputs"][number]) =>
    isContact(r) ? !o.isMine && o.address !== null && !CONTACT_PLACEHOLDERS.includes(o.address) : o.address === r.address;
  const shown =
    s.outputs.every((o) => (o.isChange ? (c.changeAddress === null ? o.isMine : o.address === c.changeAddress) : paysPayee(o))) &&
    (!isContact(r) || s.outputs.filter((o) => !o.isChange).length === 1);
  const sum = (outputs: PreparedTxSummary["outputs"]) => outputs.reduce((n, o) => n + o.amount, 0);
  const paid = sum(s.outputs.filter((o) => !o.isChange));
  const change = sum(s.outputs.filter((o) => o.isChange));
  // The total counts the payment and a custom change output; those the wallet owns stay in it.
  const back = sum(s.outputs.filter((o) => o.isMine && (!o.isChange || c.changeAddress !== null)));
  return (
    shown &&
    paid === estimate.totalSent &&
    s.totalSent === estimate.totalSent &&
    s.fee === estimate.fee &&
    change === (estimate.change ?? 0) &&
    s.totalDebit === c.total - back
  );
}

/** Create Unsigned's PSBT keeps the sheet's promise (U-05 D37): `create_unsigned` plans again, so
 * the PSBT is checked before it is copied or saved. Every output is the payment to the address
 * the sheet showed, of the amount it showed, or change of the amount it showed where it said (the
 * wallet's own, or the custom address); the fee is the confirmed one; and it spends as many coins
 * as the estimate (an unsigned PSBT's inputs are all unsigned). With the fee and every output
 * fixed, the coins' total is too; chosen coins are spent exactly by the engine (U-03 D7). */
export function unsignedKeepsPromise(c: Confirmed, a: PsbtAnalysis): boolean {
  const { estimate, recipient } = c;
  // A contact's address exists only once prepared: there is no unsigned contact payment.
  if (isContact(recipient)) return false;
  // The payment is taken out first, so a custom change address equal to the recipient's still
  // leaves the change to check.
  const paid = a.outputs.findIndex((o) => o.address === recipient.address && o.amount === estimate.totalSent);
  if (paid < 0) return false;
  const rest = a.outputs.filter((_, i) => i !== paid);
  const change =
    estimate.change === null
      ? rest.length === 0
      : rest.length === 1 && rest[0].amount === estimate.change && (c.changeAddress === null ? rest[0].isMine : rest[0].address === c.changeAddress);
  return change && a.fee === estimate.fee && a.unsignedInputs === estimate.inputCount;
}

// ---------------------------------------------------------------- the result

export type Outcome =
  /** `note`: what else went through (an accepted contact request, DP3-06 D12). */
  | { kind: "sent"; txid: string; amount: number; fee: number; note?: string }
  /** The transaction may have reached the network. `retry`: the engine still holds it, so Try
   * again repeats the broadcast. */
  | { kind: "unknown"; text: string; retry: boolean; draft: number; prepared: number; amount: number; fee: number; note?: string }
  | { kind: "failed"; text: string };

/** The engine's codes for a first broadcast that released the transaction (E0-13 D8, `released`
 * in send.rs): nothing was sent and its handle is gone. */
const RELEASED = ["send.no_peers", "send.broadcast_rejected", "send.prepared_tx_spent", "send.cancelled"] as const;

/** A contact payment's first broadcast refused before its hand-off, its transaction still held
 * (DP3-01 D20, D23): another payment to the contact is locked or under way, or its lock row could
 * not be written. Nothing was sent; the sheet abandons it. */
const CONTACT_HELD = ["contact.payment_locked", "storage"] as const;

/** The outcome of a failed broadcast of `prepared` (U-03 D10). Only a first attempt the engine
 * released, or one whose handle was already gone (never dispatched), is "not sent". Anything
 * else may have reached the network: it stays unknown, never abandoned, so the user is not
 * invited to pay twice. A retry is never released (the engine keeps an unknown transaction). */
export function broadcastFailure(
  e: unknown,
  chain: Chain,
  held: { draft: number; prepared: number; amount: number; fee: number; note?: string },
  first: boolean,
  unit: Unit = "DASH",
  contact = false,
): Outcome {
  const gone = hasCode(e, "app.unknown_handle");
  const notSent = [...RELEASED, ...(contact ? CONTACT_HELD : [])];
  if (first && (gone || notSent.some((c) => hasCode(e, c)))) return { kind: "failed", text: failureText(e, chain, unit) };
  // The handle went with the draft (a lock or network switch closed it): nothing to retry here.
  const text = gone ? "The payment may have been sent, and it can no longer be retried here." : failureText(e, chain, unit);
  return { kind: "unknown", text, retry: !gone, ...held };
}

export function failureText(e: unknown, chain: Chain, unit: Unit = "DASH"): string {
  const send = sendErrorText(e, chain, unit);
  if (send) return send.text;
  if (isAppError(e)) return e.message;
  return e instanceof Error ? e.message : String(e);
}
