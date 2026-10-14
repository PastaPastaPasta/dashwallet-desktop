// Pay ▸ To an address, the logic without React (U-03): which credential the confirm sheet
// asks for, what the engine's send refusals say and where, what the confirm sheet promised and
// whether the signed transaction keeps that promise, and how a broadcast ended.

import { hasCode, isAppError, retryAfterSecs } from "../../api/backend";
import type { AddressClass } from "../../api/generated/AddressClass";
import type { AppError } from "../../api/generated/AppError";
import type { ChangePolicy } from "../../api/generated/ChangePolicy";
import type { CoinSource } from "../../api/generated/CoinSource";
import type { FeeMode } from "../../api/generated/FeeMode";
import type { LockStatus } from "../../api/generated/LockStatus";
import type { PreparedTxSummary } from "../../api/generated/PreparedTxSummary";
import type { Recipient } from "../../api/generated/Recipient";
import type { TxEstimate } from "../../api/generated/TxEstimate";
import type { OutPoint } from "../../api/generated/OutPoint";
import { formatWithUnit, type Chain } from "../../format/amount";

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
export function sendErrorText(e: unknown, chain: Chain): { text: string; field: Field } | null {
  if (!isAppError(e)) return null;
  const amount = (duffs: number) => formatWithUnit("DASH", duffs, false, "standard", chain);
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
export type AuthRefusal = { kind: "needsPassphrase" } | { kind: "wrongPassphrase"; text: string } | { kind: "other" };

export function authRefusal(e: AppError): AuthRefusal {
  if (e.code === "vault.credential_required") return { kind: "needsPassphrase" };
  if (e.code === "vault.wrong_passphrase" || e.code === "vault.throttled") {
    const wait = retryAfterSecs(e);
    const attempts = e.data?.kind === "wrongPassphrase" ? e.data.failedAttempts : null;
    const base = e.code === "vault.throttled" ? "Too many attempts." : "The passphrase you entered was incorrect.";
    const count = attempts !== null && attempts > 1 ? ` ${attempts} failed attempts.` : "";
    return { kind: "wrongPassphrase", text: `${base}${count}${wait > 0 ? ` Try again in ${wait} s.` : ""}` };
  }
  return { kind: "other" };
}

// ---------------------------------------------------------------- confirm and check

/** What the confirm sheet showed and the user agreed to. */
export interface Confirmed {
  recipient: Recipient;
  estimate: TxEstimate;
  /** The custom change address, if any. */
  changeAddress: string | null;
  /** What leaves the wallet at most: the recipients' amount plus the fee, and the change when it
   * goes to a custom address (U-03 D21: the engine counts it if the wallet does not own the
   * address, and the renderer cannot tell). The `spend` grant's cap. */
  total: number;
}

export function confirmedOf(recipient: Recipient, estimate: TxEstimate, change: ChangePolicy): Confirmed {
  const changeAddress = change.kind === "address" ? change.address : null;
  const total = estimate.totalSent + estimate.fee + (changeAddress === null ? 0 : (estimate.change ?? 0));
  return { recipient, estimate, changeAddress, total };
}

/** The signed transaction keeps the sheet's promise (U-03 D2): it pays the address what the
 * sheet said, sends change only where the sheet said, and takes no more than the confirmed total
 * from the wallet. */
export function keepsPromise(c: Confirmed, s: PreparedTxSummary): boolean {
  const paid = s.outputs.filter((o) => !o.isChange && o.address === c.recipient.address).reduce((n, o) => n + o.amount, 0);
  const change = s.outputs.filter((o) => o.isChange);
  const changeKept = c.changeAddress === null ? change.every((o) => o.isMine) : change.every((o) => o.address === c.changeAddress);
  return paid === c.estimate.totalSent && s.totalSent === c.estimate.totalSent && changeKept && s.totalDebit <= c.total;
}

// ---------------------------------------------------------------- the result

export type Outcome =
  | { kind: "sent"; txid: string; amount: number; fee: number }
  /** The transaction may have reached the network. `retry`: the engine still holds it, so Try
   * again repeats the broadcast. */
  | { kind: "unknown"; text: string; retry: boolean; draft: number; prepared: number; amount: number; fee: number }
  | { kind: "failed"; text: string };

/** The engine's codes for a first broadcast that released the transaction (E0-13 D8, `released`
 * in send.rs): nothing was sent and its handle is gone. */
const RELEASED = ["send.no_peers", "send.broadcast_rejected", "send.prepared_tx_spent", "send.cancelled"] as const;

/** The outcome of a failed broadcast of `prepared` (U-03 D10). Only a first attempt the engine
 * released, or one whose handle was already gone (never dispatched), is "not sent". Anything
 * else may have reached the network: it stays unknown, never abandoned, so the user is not
 * invited to pay twice. A retry is never released (the engine keeps an unknown transaction). */
export function broadcastFailure(e: unknown, chain: Chain, held: { draft: number; prepared: number; amount: number; fee: number }, first: boolean): Outcome {
  const gone = hasCode(e, "app.unknown_handle");
  if (first && (gone || RELEASED.some((c) => hasCode(e, c)))) return { kind: "failed", text: failureText(e, chain) };
  // The handle went with the draft (a lock or network switch closed it): nothing to retry here.
  const text = gone ? "The payment may have been sent, and it can no longer be retried here." : failureText(e, chain);
  return { kind: "unknown", text, retry: !gone, ...held };
}

export function failureText(e: unknown, chain: Chain): string {
  const send = sendErrorText(e, chain);
  if (send) return send.text;
  if (isAppError(e)) return e.message;
  return e instanceof Error ? e.message : String(e);
}
