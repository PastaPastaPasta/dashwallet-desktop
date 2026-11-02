// Shielded funds (0.1 release plan; lane E1's contract v41 §2.13): the moves in and out of the
// wallet's shielded pool. Add to Shielded (Core → Shielded), Send (Shielded → Shielded) and Move to
// Dash wallet (Shielded → Core) share one flow: an amount (and for a send an address and a memo),
// the quote's confirm step, the call under the quote's `platformOp` grant (U-05's GrantModel), its
// progress (proving a spend takes a while) and its outcome. A lock that was not applied is finished
// with `resume`. One flow per wallet, kept while the app runs, so the sheet can close while the call
// runs and reopen to its result (as withdraw does).

import { isAppError, type Backend } from "../api/backend";
import type { AuthGrant } from "../api/generated/AuthGrant";
import type { GrantRequest } from "../api/generated/GrantRequest";
import type { ShieldedSpendOutcome } from "../api/generated/ShieldedSpendOutcome";
import type { ShieldOutcome } from "../api/generated/ShieldOutcome";
import { SHIELDED_CODES } from "../api/pending";
import { platformOp } from "../dashpay/topup";
import { failureText, Model } from "../onboarding/model";
import { moveErrorText } from "../platform/move";
import { SHIELDED_SYNCING_TEXT, syncingRefusal } from "./syncing";
import { GrantModel, PAYMENT_PROMPT } from "../tools/grant";

export type ShieldedApi = Pick<
  Backend,
  "shieldFromCore" | "resumeShield" | "shieldedSend" | "unshieldToCore" | "authorize" | "revokeGrant" | "lockStatus" | "paymentAuthentication" | "unlock" | "unlockMixingOnly"
>;

/** What a sheet is for: the three moves, and finishing a lock that was not applied. */
export type ShieldedKind = "add" | "send" | "out" | "resume";

/** What the confirm step quotes and sends. Credits are decimal strings, a duff is 1000 of them. */
export type ShieldedRequest =
  | { kind: "add"; duffs: number }
  | { kind: "send"; credits: string; to: string; memo: string | null }
  | { kind: "out"; credits: string }
  | { kind: "resume"; txid: string };

export type ShieldedStep = "form" | "confirm" | "running" | "done" | "refused";

export type ShieldedResult = { kind: "shield"; outcome: ShieldOutcome } | { kind: "spend"; outcome: ShieldedSpendOutcome };

export const SHIELDED_COPY = {
  card: {
    title: "Shielded",
    text: "A private balance. Amounts and addresses in it are not visible on the network.",
    inactive: "Shielded is not active on this network yet.",
    syncing: "Shielded is syncing. Its balance shows when it has.",
    pending: (amount: string) => `${amount} is on its way.`,
    /** A spend whose result is not confirmed: the wallet re-sends it itself, so it is never offered again. */
    unresolved: "A payment is still being confirmed. Do not send it again: the wallet checks it itself.",
    receive: "Receive",
    send: "Send",
    add: "Add to Shielded",
    out: "Move to Dash wallet",
    finish: "Finish",
    /** A lock that may already be applied: the same outpoint is read again, never a new lock. */
    checkAgain: "Check again",
    why: { empty: "Nothing in Shielded to send yet.", busy: "Another Shielded move is running." },
    /** DEC-1057: a spend of unknown result whose record was lost. Shields are not held back. */
    unrecorded: "A shielded payment from before a restart may still go through, and its record was lost. Sending or moving from Shielded asks you to confirm first.",
    unfinished: (amount: string) => `${amount} was locked to add to Shielded and is not applied yet.`,
  },
  titles: { add: "Add to Shielded", send: "Send from Shielded", out: "Move to Dash wallet", resume: "Add to Shielded", receive: "Receive in Shielded" },
  form: {
    why: { amount: "Enter an amount first.", to: "Enter a Shielded address first.", memo: "The note is too long." },
    add: "Move Dash from your wallet into Shielded. It is locked first, then Dash Platform adds it to your private balance.",
    send: "Send privately to another Shielded address.",
    out: "Move Shielded funds to your Dash wallet's next receive address.",
    amount: "Amount",
    to: "To (a Shielded address)",
    memo: "Memo (optional)",
    memoHint: "Up to 36 bytes, visible to the recipient only.",
    available: (amount: string) => `Available: ${amount}`,
    invalidAmount: "Enter an amount.",
    next: "Continue",
    cancel: "Cancel",
  },
  confirm: {
    quoting: "Working out the fee…",
    quoteFailed: "The fee could not be worked out.",
    amount: "Amount",
    fee: "Network fee",
    coreFee: "Core network fee",
    poolFee: "Platform pool fee",
    poolFeeFixed: (amount: string) => `The Platform pool fee is fixed at about ${amount}, whatever you add.`,
    total: "Total",
    totalFromWallet: "Total from your wallet",
    lock: "Amount to add",
    gain: "Added to Shielded",
    receive: "You receive about",
    to: "To",
    slow: "Making the private proof takes up to a minute. Keep Dash Wallet open.",
    slowFirst: "The first Shielded payment after starting takes longer: the prover is still getting ready.",
    back: "Back",
    go: "Confirm",
  },
  running: {
    title: "Working…",
    prove: "Proving the transaction. This takes up to a minute; keep Dash Wallet open. You can close this: Activity shows it.",
    lock: "Locking the Dash and waiting for the network. You can close this: Activity shows it.",
  },
  done: {
    add: { title: "Added to Shielded", text: (balance: string) => `Your Shielded balance is now ${balance}.` },
    waiting: { title: "Locked, not applied yet", text: "Your Dash is locked and waiting for the network's confirmation. Finish it from the Shielded card: nothing is lost." },
    checking: { title: "Checking", text: "This amount may already be in your shielded balance; we can't confirm it yet." },
    notApplied: { title: "Locked, not applied", text: (why: string) => `Your Dash is locked but Dash Platform did not add it yet (${why}). Finish it from the Shielded card: nothing is lost.` },
    send: { title: "Sent", text: (amount: string) => `${amount} was sent privately.` },
    /** The spend went out and its result is not confirmed: pending, never failed, never offered for a resend. */
    unconfirmed: { title: "Not confirmed yet", text: "Your payment went out and Dash Platform has not confirmed it yet. Activity shows it as pending until it does. Do not send it again: the wallet checks it itself." },
    out: { title: "Moved", text: (amount: string) => `About ${amount} is on its way to your Dash wallet. It arrives when Dash Platform's withdrawal is mined.` },
  },
  /** The confirmation a send or move from a wallet with an unrecorded spend needs (DEC-1057). */
  unrecorded: {
    title: "An earlier payment may still go through",
    text: "A shielded payment from before the restart may still go through, and the wallet no longer has its record. If you go on, you may pay twice.",
    go: { send: "Send anyway", out: "Move anyway" },
    back: "Go back",
    /** The engine's refusal when no confirmation came with the call. */
    refused: "A shielded payment from before the restart may still go through. Confirm to send anyway.",
  },
  close: "Close",
  show: "Show",
  unseen: "Your Shielded move has an update.",
  receive: {
    text: "Share this address to be paid privately. It does not change what the network sees of your wallet.",
    copy: "Copy address",
    notSynced: SHIELDED_SYNCING_TEXT,
    loading: "Getting your Shielded address…",
  },
} as const;

/** An error as dw-app sends it: its code (the generated `ErrorCode` lists the shielded ones) and data. */
function errorLike(e: unknown): { code: string; data: Record<string, unknown> | null } | null {
  if (!isAppError(e)) return null;
  const data = e.data;
  return { code: e.code, data: typeof data === "object" && data !== null ? (data as Record<string, unknown>) : null };
}

/** Whether `e` is the app error `code`. */
export const isCode = (e: unknown, code: string): boolean => errorLike(e)?.code === code;

/** A shielded refusal, by code. `credits` shows a credit figure as Dash, `duffs` a duff figure. */
export function shieldedErrorText(e: unknown, credits: (c: string) => string, duffs: (d: number) => string, kind?: ShieldedKind, syncing = false): string {
  const err = errorLike(e);
  if (!err) return failureText(e);
  const d = err.data;
  if (syncingRefusal(err.code, syncing)) return SHIELDED_SYNCING_TEXT;
  switch (err.code) {
    case SHIELDED_CODES.notActive:
      return SHIELDED_COPY.card.inactive;
    case SHIELDED_CODES.poolTooSmall:
      return "Shielded withdrawals open once the network's shielded pool has enough activity. Try again later.";
    case SHIELDED_CODES.invalidAddress:
      return "Enter a Shielded address of this network.";
    case SHIELDED_CODES.insufficientFunds:
      return d?.kind === "credits" ? `Not enough in Shielded: ${credits(String(d.needed))} needed with the fee, ${credits(String(d.available))} available.` : "Not enough in Shielded.";
    case SHIELDED_CODES.fundingInsufficient:
      return d?.kind === "duffs" ? `Not enough Dash in your wallet: ${duffs(Number(d.needed))} needed with the fee, ${duffs(Number(d.available))} available.` : "Not enough Dash in your wallet.";
    case SHIELDED_CODES.belowMinimum:
      // The engine's minimum is in duffs for a shield, in credits for a spend.
      if (d?.kind !== "number") return "Enter a larger amount.";
      return kind === "add" ? `Too small to add: the smallest amount is ${duffs(Number(d.value))}.` : `Too small: the smallest amount is ${credits(String(d.value))}.`;
    case SHIELDED_CODES.coinsLocked:
      return "Your Dash is reserved right now (for example by CoinJoin). Try again shortly.";
    case SHIELDED_CODES.unrecordedSpend:
      return SHIELDED_COPY.unrecorded.refused;
    case SHIELDED_CODES.tooManyInputs:
      return "That takes too many separate amounts at once. Consolidate your coins first, then try again.";
    case SHIELDED_CODES.shieldPending:
      return "An earlier add to Shielded is not finished. Finish it from the Shielded card first.";
    case "platform.context_unavailable":
      return "Waiting for the network to sync before Shielded can be used. Try again shortly.";
    case "platform.trust_mismatch":
      return "Your Shielded account could not be verified against the network. Nothing was moved.";
    case "platform.feature_off":
      return "Shielded is turned off.";
    default:
      return moveErrorText(e, credits);
  }
}

/** Back is offered when another amount or address may pass. */
export const shieldedCanChange = (e: unknown): boolean =>
  [SHIELDED_CODES.invalidAddress, SHIELDED_CODES.insufficientFunds, SHIELDED_CODES.fundingInsufficient, SHIELDED_CODES.belowMinimum, "platform.grant_exceeded", "invalid_argument"].some((c) => isCode(e, c));

const settledListeners = new Set<() => void>();

/** Called whenever a shielded call that ran has ended (events.ts re-reads `["dashpay"]` from it, so
 * the balances and Activity follow even when Home is not shown). Returns the unsubscribe. */
export function onShieldedSettled(f: () => void): () => void {
  settledListeners.add(f);
  return () => settledListeners.delete(f);
}

export class ShieldedFlow extends Model {
  /** The sheet is shown. */
  open = false;
  kind: ShieldedKind = "add";
  step: ShieldedStep = "form";
  /** The form's fields as typed (the amount in the display unit). */
  text = "";
  to = "";
  memo = "";
  request: ShieldedRequest | null = null;
  result: ShieldedResult | null = null;
  error: unknown = null;
  running = false;
  readonly grants: GrantModel;

  constructor(
    private readonly api: ShieldedApi,
    readonly walletId: string,
  ) {
    super();
    this.grants = new GrantModel(api);
    this.grants.prompt = PAYMENT_PROMPT;
    this.grants.subscribe(() => this.changed());
  }

  /** A result that came while the sheet was hidden, not yet seen. */
  get unseen(): boolean {
    return !this.open && !this.running && (this.step === "done" || this.step === "refused");
  }

  /** Opens the sheet for `kind`: at a fresh form, or where a running or unseen call is. */
  begin(kind: ShieldedKind) {
    this.open = true;
    if (!this.running && this.step !== "done" && this.step !== "refused") {
      this.kind = kind;
      this.step = "form";
      this.text = "";
      this.to = "";
      this.memo = "";
      this.request = null;
      this.result = null;
      this.error = null;
    }
    this.changed();
  }

  setText(text: string) {
    this.text = text;
    this.changed();
  }

  setTo(to: string) {
    this.to = to;
    this.changed();
  }

  setMemo(memo: string) {
    this.memo = memo;
    this.changed();
  }

  /** Form → confirm for a request the screen has checked. */
  next(request: ShieldedRequest) {
    this.request = request;
    this.step = "confirm";
    this.error = null;
    this.changed();
  }

  back() {
    if (this.running) return;
    this.step = "form";
    this.error = null;
    this.grants.cancel();
    this.changed();
  }

  /** Hides the sheet; a running call goes on and its result waits. A result seen is dropped. */
  close() {
    this.open = false;
    if (!this.running) {
      if (this.step === "done" || this.step === "refused") this.step = "form";
      this.grants.cancel();
    }
    this.changed();
  }

  /** Finishes the lock `txid` that was not applied: a grant of one duff (its one-time key), no confirm. */
  async resume(txid: string): Promise<void> {
    this.open = true;
    if (this.running) return this.changed();
    this.kind = "resume";
    this.step = "form";
    this.result = null;
    this.error = null;
    this.request = { kind: "resume", txid };
    this.changed();
    await this.run({ maxDuffs: 1, maxCredits: "0" });
  }

  /** Confirm: the quote's grant, then the call (one at a time). `confirmUnrecorded`: the user
   * confirmed going on while the wallet holds a spend whose record was lost (DEC-1057). */
  async run(grant: GrantRequest, confirmUnrecorded = false): Promise<void> {
    const request = this.request;
    if (this.running || request === null) return;
    let ran = false;
    this.running = true;
    this.error = null;
    this.changed();
    try {
      const got = await this.grants.withGrant(platformOp(grant), this.walletId, (g: AuthGrant) => {
        ran = true;
        this.step = "running";
        this.changed();
        return this.call(request, g.id, confirmUnrecorded);
      });
      if (got) {
        this.result = got.value;
        this.step = "done";
      } else if (request.kind === "resume") {
        // A cancelled passphrase: there is no form to go back to.
        this.open = false;
        this.step = "form";
      } else this.step = "confirm";
    } catch (e) {
      this.error = e;
      this.step = "refused";
    } finally {
      this.running = false;
      // A cancelled passphrase ran nothing: nothing to read again.
      if (ran) for (const f of settledListeners) queueMicrotask(f);
      this.changed();
    }
  }

  private async call(r: ShieldedRequest, grant: string, confirmUnrecorded: boolean): Promise<ShieldedResult> {
    switch (r.kind) {
      case "add":
        return { kind: "shield", outcome: await this.api.shieldFromCore(this.walletId, r.duffs, grant) };
      case "resume":
        return { kind: "shield", outcome: await this.api.resumeShield(this.walletId, r.txid, grant) };
      case "send":
        return { kind: "spend", outcome: await this.api.shieldedSend(this.walletId, r.to, r.credits, r.memo, grant, confirmUnrecorded) };
      case "out":
        return { kind: "spend", outcome: await this.api.unshieldToCore(this.walletId, r.credits, null, grant, confirmUnrecorded) };
    }
  }
}

const flows = new Map<string, ShieldedFlow>();

/** The wallet's shielded flow, created on first use and kept while the app runs. */
export function shieldedFor(api: ShieldedApi, walletId: string): ShieldedFlow {
  let f = flows.get(walletId);
  if (!f) {
    f = new ShieldedFlow(api, walletId);
    flows.set(walletId, f);
  }
  return f;
}

/** Tests: forget every flow (a fresh app). */
export function resetShielded() {
  flows.clear();
}
