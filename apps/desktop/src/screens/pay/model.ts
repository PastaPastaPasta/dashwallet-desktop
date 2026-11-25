// Pay ▸ To an address, the logic without React (U-03): which credential the confirm sheet
// asks for, what the engine's send refusals say and where, what the confirm sheet promised and
// whether the signed transaction keeps that promise, and how a broadcast ended. A payment to a
// DashPay contact (DP3-06) shares all of it; its own parts are in contact.ts.

import { errorMessage, hasCode, isAppError, retryAfterSecs } from "../../api/backend";
import { CREDITS_PER_DUFF } from "../../api/pending";
import { RESCAN_TEXT } from "../../app/rescan";
import type { AddressClass } from "../../api/generated/AddressClass";
import type { AppError } from "../../api/generated/AppError";
import type { ChangePolicy } from "../../api/generated/ChangePolicy";
import type { ContactRecipient } from "../../api/generated/ContactRecipient";
import type { CoinJoinStatus } from "../../api/generated/CoinJoinStatus";
import type { CoinSource } from "../../api/generated/CoinSource";
import type { FeeMode } from "../../api/generated/FeeMode";
import type { LockStatus } from "../../api/generated/LockStatus";
import type { PreparedTxSummary } from "../../api/generated/PreparedTxSummary";
import type { PsbtAnalysis } from "../../api/generated/PsbtAnalysis";
import type { Recipient } from "../../api/generated/Recipient";
import type { TxEstimate } from "../../api/generated/TxEstimate";
import type { OutPoint } from "../../api/generated/OutPoint";
import type { Utxo } from "../../api/generated/Utxo";
import { pendingUnproven } from "../../api/restoreHolds";
import { formatWithUnit, type Chain, type Unit } from "../../format/amount";
import { t } from "../../i18n";
import { en } from "../../i18n/en";

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

/** A mixed payment of this many coins or more warns about privacy (U-06b D3, dash-qt sendcoinsdialog.cpp). */
export const MIXED_INPUT_WARNING = 10;

/** A mixed payment whose left-over reaches this many duffs (0.01 DASH) warns, unless the 10% fee warning shows. */
export const MIXED_LEFT_OVER_WARNING = 1_000_000;

export interface MixedSendNotes {
  inputCount: number;
  manyCoins: boolean;
  /** What the fee holds beyond the rate (the estimate's `leftOver`); `null` when nothing is left over. */
  leftOver: number | null;
  /** The left-over reaches `MIXED_LEFT_OVER_WARNING` and the fee warning is not showing: the
   * warning takes the left-over note's place. */
  largeLeftOver: boolean;
}

/** What the confirm sheet says about a payment from fully mixed coins (U-06b); `null` for any other source. */
export function mixedSendNotes(coins: CoinChoice, estimate: Pick<TxEstimate, "inputCount" | "leftOver">, feeWarned: boolean): MixedSendNotes | null {
  if (coins !== "mixed") return null;
  return {
    inputCount: estimate.inputCount,
    manyCoins: estimate.inputCount >= MIXED_INPUT_WARNING,
    leftOver: estimate.leftOver > 0 ? estimate.leftOver : null,
    largeLeftOver: estimate.leftOver >= MIXED_LEFT_OVER_WARNING && !feeWarned,
  };
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
  if (pendingUnproven(u)) return t("components.coins.pendingUnproven.short");
  if (u.spendable) return null;
  if (u.reserved) return t("pay.notSelectable.reserved");
  if (u.userLocked) return t("pay.notSelectable.locked");
  if (u.isCoinbase) return t("pay.notSelectable.immature");
  return t("pay.notSelectable.notSpendable");
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

/** Pay ▸ To an address takes more than one recipient (dash-qt's Add Recipient, QT-052). A funds
 * flow still waiting on its funds ruling: on for testnet, regtest and devnet, off on mainnet. */
export const multiRecipientPay = (network: { kind: string }) => network.kind !== "mainnet";

/** Every recipient of a payment: the form's first, then the added ones (address recipients only). */
export const recipientsOf = (first: PayRecipient, more: readonly Recipient[]): PayRecipient[] => [first, ...more];

/** At most one recipient pays the fee from its amount: then what each receives is exact, the
 * subtracting one getting the rest of the estimate's total, and the sheet can show it. */
export const subtractingCount = (rs: readonly PayRecipient[]) => rs.filter(subtractsFee).length;

/** What each recipient receives, in order, from the estimate's total; `null` when more than one
 * subtracts the fee (the engine splits it, and the sheet could not show each share). */
export function receivedAmounts(rs: readonly PayRecipient[], totalSent: number): number[] | null {
  const subtracting = rs.findIndex(subtractsFee);
  if (subtractingCount(rs) > 1) return null;
  if (subtracting < 0) return rs.map((r) => r.amount);
  const others = rs.reduce((n, r, i) => (i === subtracting ? n : n + r.amount), 0);
  return rs.map((r, i) => (i === subtracting ? totalSent - others : r.amount));
}

export const subtractsFee = (r: PayRecipient) => (isContact(r) ? r.subtractFee : r.subtractFeeFromAmount);

export const withSubtractFee = (r: PayRecipient): PayRecipient => (isContact(r) ? { ...r, subtractFee: true } : { ...r, subtractFeeFromAmount: true });

/** Why a fully mixed payment without Subtract fee is refused as an absurd fee (MX-FEE): a CoinJoin
 * spend makes no change (Dash Core v24 `ONLY_FULLY_MIXED`), so what the coins hold beyond the
 * amount is fee; `null` for any other refusal or payment. */
export function mixedFeeText(e: unknown, spec: { recipient: PayRecipient; source: CoinSource }, chain: Chain, unit: Unit = "DASH"): string | null {
  if (spec.source.kind !== "fullyMixedOnly" || subtractsFee(spec.recipient) || !hasCode(e, "send.absurd_fee")) return null;
  return e.data?.kind === "fee" ? t("pay.mixedFee", { fee: formatWithUnit(unit, e.data.fee, false, "standard", chain) }) : t("pay.mixedFeeUnknown");
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
    case "platform":
      // A Platform address is paid from the Platform balance (PayFromPlatform).
      return null;
    case "shielded":
      return t("pay.address.shielded");
    case "invalid":
      return c.problem.kind === "invalidBase58Prefix" ? t("pay.address.otherNetwork") : t("pay.error.invalidAddress");
  }
}

// ---------------------------------------------------------------- the engine's refusals

export type Field = "address" | "amount" | null;

/** What a wallet that mixes has beyond what a normal payment spends. */
export interface MixHints {
  /** Fully mixed coins, spendable with Fully mixed funds only. */
  mixed: boolean;
  /** CoinJoin-account coins mixing here can still finish. */
  mixing: boolean;
  /** Denominations this wallet will not mix further, including coins restored from Dash Core. */
  stalled: boolean;
}

/** The "Available" hint's label for the coin source it counts. */
export const availableLabel = (coins: CoinChoice) => t(`pay.available.${coins}`);

/** The amount is over what a normal payment spends, on a wallet with mixed or mixing coins. */
function overNormalFunds(spendable: string, { mixed, mixing, stalled }: MixHints): string {
  const parts = [t("pay.overNormal.exceeds", { spendable })];
  if (mixed) parts.push(t("pay.overNormal.mixed"));
  if (mixing) {
    parts.push(
      mixed
        ? t("pay.overNormal.mixingWithMixed")
        : t("pay.overNormal.mixing"),
    );
  }
  if (stalled) {
    parts.push(
      mixed
        ? t("pay.overNormal.stalledWithMixed")
        : mixing
          ? t("pay.overNormal.stalledWithMixing")
          : t("pay.overNormal.stalled"),
    );
  }
  return parts.join(" ");
}

/** dash-qt's send-dialog text for a refusal (`SendCoinsDialog::processSendCoinsReturn`), and the
 * field it belongs to; `null` when the error is not a send refusal. */
export function sendErrorText(e: unknown, chain: Chain, unit: Unit = "DASH", mix?: MixHints): { text: string; field: Field } | null {
  if (!isAppError(e)) return null;
  const amount = (duffs: number) => formatWithUnit(unit, duffs, false, "standard", chain);
  switch (e.code) {
    case "send.no_recipients":
      return { text: t("pay.error.noRecipients"), field: null };
    case "send.invalid_address":
    case "invalid_address":
      return { text: t("pay.error.invalidAddress"), field: "address" };
    case "send.platform_address":
      return { text: t("pay.error.platformAddress"), field: "address" };
    case "send.duplicate_address":
      return { text: t("pay.error.duplicateAddress"), field: "address" };
    case "send.invalid_amount":
      return { text: t("pay.error.invalidAmount"), field: "amount" };
    case "send.dust_amount":
      return { text: t("pay.error.dustAmount"), field: "amount" };
    case "send.amount_too_small_after_fee":
      return { text: t("pay.error.tooSmallAfterFee"), field: "amount" };
    case "send.amount_exceeds_balance":
      return { text: e.data?.kind === "duffs" && mix && (mix.mixed || mix.mixing || mix.stalled) ? overNormalFunds(amount(e.data.available), mix) : t("pay.error.exceedsBalance"), field: "amount" };
    case "send.amount_with_fee_exceeds_balance": {
      const fee = e.data?.kind === "fee" ? e.data.fee : null;
      return {
        text: fee === null ? t("pay.error.withFeeExceedsBalance") : t("pay.error.withFeeExceedsBalanceFee", { fee: amount(fee) }),
        field: "amount",
      };
    }
    case "send.insufficient_mixed_funds": {
      const available = e.data?.kind === "duffs" ? e.data.available : null;
      return { text: available === null ? t("pay.error.insufficientMixed") : t("pay.error.insufficientMixedAvailable", { available: amount(available) }), field: "amount" };
    }
    case "send.outpoint_unavailable":
      return { text: t("pay.error.outpointUnavailable"), field: null };
    case "send.absurd_fee": {
      const fee = e.data?.kind === "fee" ? e.data.fee : null;
      return { text: fee === null ? t("pay.error.absurdFee") : t("pay.error.absurdFeeAmount", { fee: amount(fee) }), field: null };
    }
    case "send.tx_too_large":
      return { text: t("pay.error.txTooLarge"), field: null };
    case "send.invalid_change_address":
      return { text: t("pay.error.invalidChangeAddress"), field: null };
    case "send.watch_only":
      return { text: t("pay.error.watchOnly"), field: null };
    case "send.vault_locked":
    case "vault.locked":
      return { text: t("pay.error.vaultLocked"), field: null };
    case "send.grant_invalid":
    case "vault.grant_invalid":
      return { text: t("pay.error.grantInvalid"), field: null };
    case "send.grant_exceeded":
      return { text: t("pay.error.changed"), field: null };
    case "send.prepared_tx_spent":
      return { text: t("pay.error.preparedSpent"), field: null };
    case "send.no_peers":
      return { text: t("pay.error.noPeers"), field: null };
    case "rescan_in_progress":
      return { text: RESCAN_TEXT.running, field: null };
    case "rescan_interrupted":
      return { text: RESCAN_TEXT.interrupted, field: null };
    case "send.broadcast_rejected":
      return {
        text: e.data?.kind === "reason" ? t("pay.error.rejectedReason", { reason: e.data.reason }) : t("pay.error.rejected"),
        field: null,
      };
    case "send.broadcast_unknown":
      return { text: t("pay.error.broadcastUnknown"), field: null };
    case "send.cancelled":
      return { text: t("pay.error.cancelled"), field: null };
    case "send.held_until_restart":
      return { text: t("pay.error.heldUntilRestart"), field: null };
    // A payment to a contact (DP3-06 D13).
    case "contact.payment_locked":
      return { text: t("pay.error.contactLocked"), field: null };
    case "contact.channel_broken":
      return { text: t("contacts.pane.broken"), field: null };
    case "contact.ineligible":
      return {
        text: e.data?.kind === "ineligible" && e.data.reason === "pendingIncoming" ? t("pay.error.pendingIncoming") : t("pay.error.ineligible"),
        field: null,
      };
    case "contact.self":
      return { text: t("pay.error.self"), field: null };
    case "identity.not_found":
      return { text: t("pay.error.noIdentity"), field: null };
    case "platform.unavailable":
    case "platform.timeout":
      return { text: t("pay.error.platformUnreachable"), field: null };
    case "platform.lease_revoked":
      return { text: t("pay.error.leaseRevoked"), field: null };
    case "platform.lease_expired":
    case "platform.grant_invalid":
      return { text: t("pay.error.grantInvalid"), field: null };
    case "platform.needs_grant":
    case "platform.signer_unavailable":
      return { text: t("pay.error.unlockAndRetry"), field: null };
    case "platform.grant_exceeded":
      return { text: t("pay.error.changed"), field: null };
    case "storage":
      return { text: t("pay.error.storage"), field: null };
    default:
      return null;
  }
}

/** A contact payment's refusals over the contact's data (H-09): `prepare` waits for it to verify,
 * then refuses with `platform.context_unavailable`, or `platform.trust_mismatch` when it does not
 * match. `null` for any other refusal; other Platform calls keep their own words for these. */
export function contactDataText(e: unknown): string | null {
  if (hasCode(e, "platform.context_unavailable")) return t("pay.error.contactUnchecked");
  if (hasCode(e, "platform.trust_mismatch")) return t("pay.error.contactMismatch");
  return null;
}

/** A notice the Pay form shows in place of an error, with Try again. */
export type PayNotice = { kind: "updating"; text: string };

/** A contact payment refused with `platform.context_unavailable`: after a launch, until the first
 * DashPay pass re-checks the contact (H-09). Not an error: a notice whose Try again runs the check
 * again. `accepted`: what an accept that went through says first. `null` for any other refusal,
 * and for a payment to an address, which keep their own words. */
export function contactsUpdatingNotice(e: unknown, contact: boolean, accepted: string | null = null): PayNotice | null {
  if (!contact || !hasCode(e, "platform.context_unavailable")) return null;
  const text = t("pay.notice.contactsUpdating");
  return { kind: "updating", text: accepted ? `${accepted} ${text}` : text };
}

/** A refused estimate: the reason under the form, or the notice that replaces it. `refusal`: the
 * send refusal's own text, if it has one. */
export function estimateRefusal(e: unknown, contact: boolean, refusal: string | null): { general: string | null; notice: PayNotice | null } {
  const notice = contactsUpdatingNotice(e, contact);
  if (notice) return { general: null, notice };
  return { general: refusal ?? (contact ? contactDataText(e) : null) ?? errorMessage(e), notice: null };
}

// ---------------------------------------------------------------- the payment policy (E0-13 D11)

/** What the signing window would cover for this payment. `null` when it has not been read. */
export interface SigningCover {
  open: boolean;
  /** What the window can still authorize, in duffs. */
  limitDuffs: number;
  /** What this payment counts against that limit, in duffs. */
  amount: number | bigint;
}

/** What "Accept and pay" counts against the window: the payment and the accept's quoted Platform
 * value, its credits rounded up to whole duffs as dw-vault's `quick_unlock_value` does. Exact
 * past 2^53. */
export function acceptAndPayValue(total: number, quote: { maxDuffs: number; maxCredits: string }): bigint {
  const credits = BigInt(quote.maxCredits);
  return BigInt(total) + BigInt(quote.maxDuffs) + (credits + CREDITS_PER_DUFF - 1n) / CREDITS_PER_DUFF;
}

/** The credential the confirm sheet asks for. An encrypted vault asks when "Ask for my passphrase
 * for every payment" is on or not yet known, and when the post-unlock window does not cover this
 * amount. The engine enforces the same rule. */
export function credentialPrompt(
  lock: LockStatus | undefined,
  paymentAuth: boolean | undefined,
  cover?: SigningCover | null,
): "passphrase" | "none" {
  if (lock === undefined) return "passphrase";
  if (!lock.encrypted) return "none";
  if (paymentAuth !== false) return "passphrase";
  if (!cover || !cover.open || BigInt(cover.amount) > BigInt(cover.limitDuffs)) return "passphrase";
  return "none";
}

/** What the confirm sheet offers (P15, U-05 D18). Advanced's unsigned path, or a wallet that
 * cannot sign, makes a PSBT and nothing else; Send waits until the wallet is known to sign. */
export function confirmActions(unsignedAsked: boolean, watchOnly: boolean | undefined): { send: boolean; credential: boolean; unsignedFirst: boolean } {
  const send = !unsignedAsked && watchOnly === false;
  return { send, credential: send, unsignedFirst: unsignedAsked || watchOnly === true };
}

/** A contact's confirm sheet (#82) has no unsigned path: it sends once the wallet is known to sign,
 * and otherwise offers nothing. */
export function contactConfirmActions(watchOnly: boolean | undefined): ReturnType<typeof confirmActions> {
  const send = watchOnly === false;
  return { send, credential: send, unsignedFirst: false };
}

export const PAY_INCOMPLETE = en.pay.incomplete;
export const PAY_INCOMPLETE_CONTACT = en.pay.incompleteContact;
export const PAY_CHECKING = en.pay.checking;

/** The form a payment is drafted in: an address and an amount, or a picked contact and an amount. */
export type PayForm = "address" | "contact";

/** What an incomplete form still needs: the contact form has no address to ask for. */
export const incompleteText = (form: PayForm) => (form === "contact" ? t("pay.incompleteContact") : t("pay.incomplete"));

/** Why a payment cannot be reviewed yet (P15): the first field problem the form shows, then the
 * draft's own, then whether it is still incomplete or being checked; `null` once it can. */
export function reviewBlockedReason(ready: boolean, problems: (string | null | undefined)[], complete: boolean, form: PayForm = "address"): string | null {
  if (ready) return null;
  return problems.find((p): p is string => !!p) ?? (complete ? t("pay.checking") : incompleteText(form));
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
    const base = e.code === "vault.throttled" ? t("pay.auth.throttled") : t("pay.auth.wrongPassphrase");
    const count = attempts !== null && attempts > 1 ? ` ${t("pay.auth.failedAttempts", { count: attempts })}` : "";
    return { kind: "wrongPassphrase", text: `${base}${count}`, waitSecs: wait };
  }
  return { kind: "other" };
}

// ---------------------------------------------------------------- confirm and check

/** What the confirm sheet showed and the user agreed to. */
export interface Confirmed {
  recipient: PayRecipient;
  /** Recipients after the first (Add Recipient): address recipients, paid in the same transaction. */
  more: Recipient[];
  estimate: TxEstimate;
  /** The custom change address, if any. */
  changeAddress: string | null;
  /** What leaves the wallet at most: the recipients' amount plus the fee, and the change when it
   * goes to a custom address (U-03 D21: the engine counts it if the wallet does not own the
   * address, and the renderer cannot tell). The `spend` grant's cap. */
  total: number;
}

export function confirmedOf(recipient: PayRecipient, estimate: TxEstimate, change: ChangePolicy, more: Recipient[] = []): Confirmed {
  const changeAddress = change.kind === "address" ? change.address : null;
  const total = estimate.totalSent + estimate.fee + (changeAddress === null ? 0 : (estimate.change ?? 0));
  return { recipient, more, estimate, changeAddress, total };
}

/** Takes, for each address recipient in order, the one output paying its address exactly what it
 * receives; the outputs left over, or `null` when one is missing. */
function takePayments<O extends { address: string | null; amount: number }>(outputs: readonly O[], rs: readonly Recipient[], received: readonly number[]): O[] | null {
  const rest = [...outputs];
  for (const [i, r] of rs.entries()) {
    const at = rest.findIndex((o) => o.address === r.address && o.amount === received[i]);
    if (at < 0) return null;
    rest.splice(at, 1);
  }
  return rest;
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
  if (c.more.length > 0) return keepsManyPromise(c, s);
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
    // As many coins as the sheet counted: denominations can make the same total from other coins,
    // ten mixed ones where it showed one (the input count is what the mixed-coin warnings read).
    s.inputs.length === estimate.inputCount &&
    paid === estimate.totalSent &&
    s.totalSent === estimate.totalSent &&
    s.fee === estimate.fee &&
    change === (estimate.change ?? 0) &&
    s.totalDebit === c.total - back
  );
}

/** `keepsPromise` for several address recipients: each is paid exactly what the sheet showed
 * (`receivedAmounts`), by one output to its address; every other output is change where the sheet
 * said; and the fee, the change and the debit are the confirmed ones. Two recipients subtracting
 * the fee fail closed: the sheet could not show what each receives. */
function keepsManyPromise(c: Confirmed, s: PreparedTxSummary): boolean {
  const { estimate, recipient: first } = c;
  if (isContact(first)) return false;
  const rs = [first, ...c.more];
  const received = receivedAmounts(rs, estimate.totalSent);
  if (!received || new Set(rs.map((r) => r.address)).size !== rs.length) return false;
  const paidOutputs = s.outputs.filter((o) => !o.isChange);
  if (paidOutputs.length !== rs.length || takePayments(paidOutputs, rs, received)?.length !== 0) return false;
  const changeOk = s.outputs.filter((o) => o.isChange).every((o) => (c.changeAddress === null ? o.isMine : o.address === c.changeAddress));
  const sum = (outputs: PreparedTxSummary["outputs"]) => outputs.reduce((n, o) => n + o.amount, 0);
  const change = sum(s.outputs.filter((o) => o.isChange));
  const back = sum(s.outputs.filter((o) => o.isMine && (!o.isChange || c.changeAddress !== null)));
  return (
    changeOk &&
    sum(paidOutputs) === estimate.totalSent &&
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
  // The payments are taken out first, so a custom change address equal to a recipient's still
  // leaves the change to check.
  const rs = [recipient, ...c.more];
  const received = receivedAmounts(rs, estimate.totalSent);
  if (!received || new Set(rs.map((r) => r.address)).size !== rs.length) return false;
  // The payments add up to what the sheet showed as sent (a planner's estimate that disagrees is no promise kept).
  if (received.reduce((n, a) => n + a, 0) !== estimate.totalSent) return false;
  const rest = takePayments(a.outputs, rs, received);
  if (rest === null) return false;
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
  /** The transaction `txid` may have reached the network. `retry`: the engine still holds it, so
   * Try again repeats the broadcast. */
  | { kind: "unknown"; text: string; retry: boolean; txid: string; draft: number; prepared: number; amount: number; fee: number; note?: string }
  | { kind: "failed"; text: string };

/** A signed payment handed to the broadcast, its outcome not yet known. */
export type Sending = { amount: number; fee: number; note?: string };

/** The engine's codes for a first broadcast that released the transaction (E0-13 D8, `released`
 * in send.rs): nothing was sent and its handle is gone. */
const RELEASED = ["send.no_peers", "send.broadcast_rejected", "send.prepared_tx_spent", "send.cancelled"] as const;

/** A contact payment's first broadcast refused before its hand-off, its transaction still held
 * (DP3-01 D20, D23): another payment to the contact is locked or under way. Nothing was sent; the
 * sheet abandons it. */
const CONTACT_HELD = ["contact.payment_locked"] as const;

/** A first broadcast whose record could not be written: refused before its hand-off, so nothing
 * was sent, and the engine refuses a retry of it. The sheet abandons it. */
export const UNRECORDED = "storage" as const;

/** A first broadcast that was not sent: `abandon`, where the engine still holds the transaction,
 * then `notSent`. A payment held until a restart refuses its abandon; it is reported not sent all
 * the same. */
export async function settleNotSent(abandon: (() => Promise<unknown>) | null, notSent: () => void): Promise<void> {
  if (abandon) await abandon().catch(() => undefined);
  notSent();
}

/** The outcome of a failed broadcast of `prepared` (U-03 D10). Only a first attempt the engine
 * released, or one whose handle was already gone (never dispatched), is "not sent". Anything
 * else may have reached the network: it stays unknown, never abandoned, so the user is not
 * invited to pay twice. A retry is never released (the engine keeps an unknown transaction). */
export function broadcastFailure(
  e: unknown,
  chain: Chain,
  held: { txid: string; draft: number; prepared: number; amount: number; fee: number; note?: string },
  first: boolean,
  unit: Unit = "DASH",
  contact = false,
): Outcome {
  // Not sent, but its coins stay reserved until a restart: no retry, which would send it.
  if (hasCode(e, "send.held_until_restart")) return { kind: "failed", text: failureText(e, chain, unit) };
  const gone = hasCode(e, "app.unknown_handle");
  const notSent = [...RELEASED, UNRECORDED, ...(contact ? CONTACT_HELD : [])];
  if (first && (gone || notSent.some((c) => hasCode(e, c)))) return { kind: "failed", text: failureText(e, chain, unit) };
  // The handle went with the draft (a lock or network switch closed it): nothing to retry here.
  const text = gone ? t("pay.error.noRetryHere") : failureText(e, chain, unit);
  return { kind: "unknown", text, retry: !gone, ...held };
}

/** `contact`: the payment pays a DashPay contact, whose data refusals say so (`contactDataText`). */
export function failureText(e: unknown, chain: Chain, unit: Unit = "DASH", contact = false): string {
  const data = contact ? contactDataText(e) : null;
  if (data) return data;
  const send = sendErrorText(e, chain, unit);
  if (send) return send.text;
  if (isAppError(e)) return e.message;
  return e instanceof Error ? e.message : String(e);
}

/** The confirm sheet says the payment is not private (lane F): a normal or selected payment while
 * mixing runs, or with fully mixed coins in the wallet, that spends a coin which is not mixed at
 * all. Automatic selection never spends a denomination (dw-engine `auto_selectable`), so every
 * input of "Any available funds" is unmixed. Coin control spends exactly the selected coins: one
 * that is not a denomination is unmixed. A denomination is mixed, at least partly, or its rounds
 * are unknown, so it says nothing of it. A fully mixed payment never says it. */
export function paysWithUnmixed(advanced: Advanced, coins: readonly Utxo[] | undefined, status: CoinJoinStatus | undefined): boolean {
  if (!status || advanced.coins === "mixed") return false;
  if (status.state === "idle" && status.balances.fullyMixed === 0) return false;
  if (advanced.coins === "any") return true;
  if (!coins) return false;
  const byKey = new Map(coins.map((c) => [outpointKey(c.outpoint), c]));
  return advanced.selected.some((k) => byKey.get(k)?.coinjoinDenominated === false);
}
