// Tools ▸ PSBT (QT-076…079): the Swift `PSBTViewModel`, ported. Load a PSBT from a file (binary
// or base64, under 100 MiB) or a pasted base64 text (U-05 D16), or take the one Pay's Create
// Unsigned made (D18); dash-qt's PSBT Operations lines and status; Sign under a spend grant that
// covers what leaves the wallet (D17); Broadcast; Copy; Save as a binary `.psbt` (D2).

import { errorMessage, hasCode, isAppError, type Backend } from "../api/backend";
import type { OpenedFile } from "../api/generated/OpenedFile";
import type { PsbtAnalysis } from "../api/generated/PsbtAnalysis";
import { base64ToBytes, saveBase64 } from "../app/files";
import { formatWithUnit, type Chain } from "../format/amount";
import { Model } from "../onboarding/model";
import type { GrantModel } from "./grant";

export const PsbtText = {
  sends: (amount: string, to: string) => ` * Sends ${amount} to ${to}`,
  ownAddress: "own address",
  unknownAddress: "unknown address",
  feeUnknown: "Unable to calculate transaction fee or total transaction amount.",
  paysFee: (fee: string) => `Pays transaction fee: ${fee}`,
  total: (amount: string) => `Total Amount: ${amount}`,
  unsignedInputs: (n: number) => `Transaction has ${n} unsigned inputs.`,
  missingInputInfo: "Transaction is missing some information about inputs.",
  needsSignatures: "Transaction still needs signature(s).",
  noWallet: "(But no wallet is loaded.)",
  cannotSign: "(But this wallet cannot sign transactions.)",
  noMatchingKeys: "(But this wallet does not have the right keys.)",
  complete: "Transaction is fully signed and ready for broadcast.",
  signedComplete: "Signed transaction successfully. Transaction is ready to broadcast.",
  signedPartially: (n: number) => `Signed ${n} inputs, but more signatures are still required.`,
  couldNotSign: "Could not sign any more inputs.",
  broadcastSucceeded: (txid: string) => `Transaction broadcast successfully! Transaction ID: ${txid}`,
  broadcastFailed: (why: string) => `Transaction broadcast failed: ${why}`,
  copied: "PSBT copied to clipboard.",
  saved: "PSBT saved to disk.",
  saveFailed: "There was an error trying to save the data to this location.",
  loadFailed: (why: string) => `Failed to load transaction: ${why}`,
  pasteInvalid: "Unable to decode PSBT (invalid base64)",
  unsignedCopied: "The PSBT has been copied to the clipboard. You can also save it.",
  noExternalAmount: "The amount this transaction sends could not be determined.",
  invalid: "Unable to decode PSBT",
  tooLarge: "PSBT file must be smaller than 100 MiB",
};

/** Copy for the `psbt.*` refusals (the Swift `L10n.M2Errors`). */
export function psbtErrorText(e: unknown): string {
  if (!isAppError(e)) return errorMessage(e);
  switch (e.code) {
    case "psbt.invalid":
      return PsbtText.invalid;
    case "psbt.too_large":
    case "app.file_too_large":
      return PsbtText.tooLarge;
    case "psbt.not_complete":
      return "Transaction is not fully signed yet.";
    case "psbt.fee_rate_too_high":
      return "The fee rate is above the broadcast limit of 0.1 DASH/kB.";
    case "psbt.fee_unknown":
      return "The fee cannot be determined: the PSBT lacks the previous transaction of an input.";
    case "psbt.absurd_fee":
      return "The fee is above 0.1 DASH and is considered an absurdly high fee.";
    case "psbt.watch_only":
      return "This wallet cannot sign transactions.";
    case "psbt.grant_exceeded":
      return "The amount is more than was authorized. Please try again.";
    case "psbt.no_peers":
      return "No peers are connected. Try again when the wallet is connected to the network.";
    case "psbt.broadcast_rejected":
      return "The network rejected the transaction.";
    case "psbt.broadcast_unknown":
      return "The transaction was handed to the network, but whether it was accepted is unknown. Check the transaction list before trying again.";
    case "vault.locked":
    case "vault.mixing_only":
      return "Cannot sign inputs while wallet is locked.";
    case "app.file_read":
      return "The file could not be read.";
    default:
      return errorMessage(e);
  }
}

const MAGIC = [0x70, 0x73, 0x62, 0x74, 0xff];

/** A loaded file as the base64 PSBT the engine takes: a binary PSBT (`psbt\xff…`) is the file's
 * bytes, anything else is read as base64 text. */
export function psbtOfFile(file: OpenedFile): string {
  const head = base64ToBytes(file.data.slice(0, 8));
  if (MAGIC.every((b, i) => head[i] === b)) return file.data;
  return new TextDecoder().decode(base64ToBytes(file.data)).trim();
}

const BASE64 = /^[A-Za-z0-9+/]+={0,2}$/;

export type PsbtStep = "empty" | "loading" | "ready" | "signing" | "broadcasting" | "broadcast";

type Api = Pick<Backend, "analyzePsbt" | "signPsbt" | "broadcastPsbt" | "saveFileAs">;

export class PsbtModel extends Model {
  step: PsbtStep = "empty";
  /** The PSBT, base64. */
  psbt: string | null = null;
  analysis: PsbtAnalysis | null = null;
  /** dash-qt's result line after sign, broadcast, copy or save. */
  message: string | null = null;
  error: string | null = null;
  /** The broadcast's txid; or `true` once a broadcast's outcome is unknown, which offers no
   * second broadcast (D17). */
  sent: string | true | null = null;

  constructor(
    private readonly api: Api,
    readonly grants: GrantModel,
    /** The wallet that signs; `null`: no wallet is loaded. */
    readonly walletId: string | null,
    private readonly chain: Chain,
  ) {
    super();
  }

  #amount(duffs: number) {
    return formatWithUnit("DASH", duffs, false, "standard", this.chain);
  }

  /** dash-qt `renderTransaction`: one line per output, the fee and total, unsigned inputs. */
  get lines(): string[] {
    const a = this.analysis;
    if (!a) return [];
    const lines = a.outputs.map((o) => PsbtText.sends(this.#amount(o.amount), o.address ?? PsbtText.unknownAddress) + (o.isMine ? ` (${PsbtText.ownAddress})` : ""));
    if (a.fee !== null) {
      lines.push(PsbtText.paysFee(this.#amount(a.fee)));
      if (a.total !== null) lines.push(PsbtText.total(this.#amount(a.total)));
    } else lines.push(PsbtText.feeUnknown);
    if (a.unsignedInputs > 0) lines.push(PsbtText.unsignedInputs(a.unsignedInputs));
    return lines;
  }

  /** dash-qt's status line for the PSBT's role and this wallet. */
  get status(): string | null {
    const a = this.analysis;
    if (!a) return null;
    if (a.status === "missingInputInfo") return PsbtText.missingInputInfo;
    if (a.status === "complete") return PsbtText.complete;
    const but = { noWallet: PsbtText.noWallet, watchOnly: PsbtText.cannotSign, noMatchingKeys: PsbtText.noMatchingKeys, canSign: null }[a.signability];
    return but ? `${PsbtText.needsSignatures} ${but}` : PsbtText.needsSignatures;
  }

  get canSign(): boolean {
    return this.step === "ready" && this.walletId !== null && this.analysis?.status === "needsSignatures" && this.analysis.signability === "canSign";
  }

  get canBroadcast(): boolean {
    return this.step === "ready" && this.sent === null && this.analysis?.status === "complete";
  }

  /** dash-qt's Save suggestion: `<address>-<amount>` per paid output, `.psbt`. */
  get suggestedFileName(): string {
    const paid = (this.analysis?.outputs ?? []).filter((o) => !o.isMine);
    const parts = paid.map((o) => `${o.address ?? PsbtText.unknownAddress}-${this.#amount(o.amount)}`);
    return (parts.length ? parts.join(" - ") : "transaction") + ".psbt";
  }

  /** A file the open dialog read. */
  async loadFile(file: OpenedFile) {
    let psbt: string;
    try {
      psbt = psbtOfFile(file);
    } catch {
      return this.#loadFailed(PsbtText.invalid);
    }
    await this.#open(psbt);
  }

  /** A failed open dialog (too large, unreadable). */
  fileFailed(e: unknown) {
    this.#loadFailed(psbtErrorText(e));
  }

  /** A pasted base64 PSBT. */
  async loadText(text: string) {
    const t = text.replace(/\s+/g, "");
    if (t === "" || !BASE64.test(t)) return this.#fail(PsbtText.pasteInvalid);
    await this.#open(t);
  }

  /** Pay's Create Unsigned (QT-077): the PSBT, and whether it is on the clipboard already. */
  async loadUnsigned(psbt: string, copied: boolean) {
    if ((await this.#open(psbt)) && copied) this.#say(PsbtText.unsignedCopied);
  }

  async sign() {
    const a = this.analysis;
    if (!this.canSign || !a || !this.psbt || this.walletId === null) return;
    // `null` while the fee is unknown; the engine refuses to sign then (review H1).
    if (a.total === null) return this.#fail(PsbtText.noExternalAmount);
    this.#to("signing");
    let grant;
    try {
      grant = await this.grants.obtain({ kind: "spend", maxDuffs: a.total }, this.walletId);
    } catch (e) {
      this.#to("ready");
      return this.#fail(psbtErrorText(e));
    }
    if (grant === null) return this.#to("ready");
    try {
      const signed = await this.api.signPsbt(this.walletId, this.psbt, grant.id);
      const after = await this.api.analyzePsbt(this.walletId, signed);
      this.psbt = signed;
      this.analysis = after;
      this.error = null;
      this.message =
        after.status === "complete"
          ? PsbtText.signedComplete
          : after.unsignedInputs < a.unsignedInputs
            ? PsbtText.signedPartially(a.unsignedInputs - after.unsignedInputs)
            : PsbtText.couldNotSign;
    } catch (e) {
      this.grants.revoke(grant);
      this.error = psbtErrorText(e);
    }
    this.#to("ready");
  }

  async broadcast() {
    if (!this.canBroadcast || !this.psbt) return;
    this.#to("broadcasting");
    this.error = null;
    this.message = null;
    try {
      const txid = await this.api.broadcastPsbt(this.psbt);
      this.sent = txid;
      this.message = PsbtText.broadcastSucceeded(txid);
      this.#to("broadcast");
    } catch (e) {
      if (hasCode(e, "psbt.broadcast_unknown")) this.sent = true;
      this.error = PsbtText.broadcastFailed(psbtErrorText(e));
      this.#to("ready");
    }
  }

  /** The screen copied `psbt`. */
  copied() {
    this.#say(PsbtText.copied);
  }

  /** Save…: the binary PSBT, owner-only, where the save dialog says. */
  async save() {
    if (!this.psbt) return;
    try {
      if ((await saveBase64(this.api, "psbt", this.suggestedFileName, this.psbt)) !== null) this.#say(PsbtText.saved);
    } catch {
      this.#fail(PsbtText.saveFailed);
    }
  }

  close() {
    this.psbt = null;
    this.analysis = null;
    this.message = null;
    this.error = null;
    this.sent = null;
    this.#to("empty");
  }

  async #open(psbt: string): Promise<boolean> {
    const had = this.psbt !== null;
    this.step = "loading";
    this.message = null;
    this.error = null;
    this.changed();
    try {
      this.analysis = await this.api.analyzePsbt(this.walletId, psbt);
      this.psbt = psbt;
      this.sent = null;
      this.#to("ready");
      return true;
    } catch (e) {
      const why = psbtErrorText(e);
      this.error = hasCode(e, "psbt.invalid") || hasCode(e, "psbt.too_large") ? PsbtText.loadFailed(why) : why;
      this.#to(had ? "ready" : "empty");
      return false;
    }
  }

  #loadFailed(why: string) {
    this.#fail(PsbtText.loadFailed(why));
  }

  #fail(text: string) {
    this.error = text;
    this.message = null;
    this.changed();
  }

  #say(text: string) {
    this.message = text;
    this.error = null;
    this.changed();
  }

  #to(step: PsbtStep) {
    this.step = step;
    this.changed();
  }
}
