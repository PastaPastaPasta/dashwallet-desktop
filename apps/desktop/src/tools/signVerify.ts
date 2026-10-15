// Tools ▸ Sign/Verify (QT-099, QT-100): the Swift `SignVerifyViewModel`, ported. Sign checks the
// address first with dash-qt's texts, then asks for a `signMessage` grant (U-05 D15); a grant the
// signature did not use is revoked. Verify shows the engine's outcome and dash-qt's text. A result
// belongs to the inputs it was made from: an edit clears it, and a reply to older inputs is dropped
// (U-05 D38).

import { isAppError, type Backend } from "../api/backend";
import type { VerifyMessageResult } from "../api/generated/VerifyMessageResult";
import { failureText, Model } from "../onboarding/model";
import type { GrantModel } from "./grant";

export const SignVerifyText = {
  invalidAddress: "The entered address is invalid. Please check the address and try again.",
  addressNoKey: "The entered address does not refer to a key. Please check the address and try again.",
  privateKeyUnavailable: "Private key for the entered address is not available.",
  signingFailed: "Message signing failed.",
  signed: "Message signed.",
  unlockCancelled: "Wallet unlock was cancelled.",
  changedWhileSigning: "The address or message changed while signing. Sign the message again.",
};

/** dash-qt's text for a failed signature. */
export function signErrorText(e: unknown): string {
  if (!isAppError(e)) return SignVerifyText.signingFailed;
  switch (e.code) {
    case "invalid_address":
      return SignVerifyText.invalidAddress;
    case "address_no_key":
      return SignVerifyText.addressNoKey;
    case "address_not_mine":
      return SignVerifyText.privateKeyUnavailable;
    case "vault.locked":
    case "vault.mixing_only":
      return SignVerifyText.unlockCancelled;
    default:
      return SignVerifyText.signingFailed;
  }
}

export type SignResult = { ok: true; text: string } | { ok: false; text: string };

type Api = Pick<Backend, "classifyAddress" | "signMessage" | "verifyMessage">;

export class SignVerifyModel extends Model {
  signature = "";
  signResult: SignResult | null = null;
  verifyResult: VerifyMessageResult | null = null;
  signing = false;
  // Bumped by every sign/verify and every edit; a reply carrying an older number is stale.
  #signInputs = 0;
  #verifyInputs = 0;

  constructor(
    private readonly api: Api,
    readonly grants: GrantModel,
    readonly walletId: string,
    private readonly network: string,
  ) {
    super();
  }

  get verified(): boolean {
    return this.verifyResult?.outcome === "verified";
  }

  async sign(address: string, message: string) {
    if (this.signing) return;
    const a = address.trim();
    // Busy before the first await: a second submit meanwhile must not sign twice.
    this.signing = true;
    const since = this.grants.epoch;
    const inputs = ++this.#signInputs;
    this.signature = "";
    this.signResult = null;
    this.changed();
    try {
      const kind = await this.api.classifyAddress(this.network, a).catch(() => null);
      if (kind?.kind !== "core") return this.#signed(inputs, { ok: false, text: SignVerifyText.invalidAddress });
      if (kind.scriptHash) return this.#signed(inputs, { ok: false, text: SignVerifyText.addressNoKey });
      let grant;
      try {
        grant = await this.grants.obtain({ kind: "signMessage" }, this.walletId, since);
      } catch (e) {
        return this.#signed(inputs, { ok: false, text: failureText(e) });
      }
      if (grant === null) return this.#signed(inputs, { ok: false, text: SignVerifyText.unlockCancelled });
      // Edited while the passphrase sheet was open: the message the user left is not signed.
      if (inputs !== this.#signInputs) {
        this.grants.revoke(grant);
        return this.#signed(inputs, { ok: false, text: SignVerifyText.changedWhileSigning });
      }
      try {
        const signature = await this.api.signMessage(this.walletId, a, message, grant.id);
        this.#signed(inputs, { ok: true, text: SignVerifyText.signed }, signature);
      } catch (e) {
        this.grants.revoke(grant);
        this.#signed(inputs, { ok: false, text: signErrorText(e) });
      }
    } finally {
      this.signing = false;
      this.changed();
    }
  }

  /** The address or message changed, or Clear All: the signature no longer describes them. */
  clearSign() {
    this.#signInputs += 1;
    if (this.signature === "" && this.signResult === null) return;
    this.signature = "";
    this.signResult = null;
    this.changed();
  }

  async verify(address: string, signature: string, message: string) {
    const inputs = ++this.#verifyInputs;
    this.verifyResult = null;
    this.changed();
    let result: VerifyMessageResult;
    try {
      result = await this.api.verifyMessage(address.trim(), signature.trim(), message);
    } catch (e) {
      result = { outcome: "invalidAddress", text: failureText(e) };
    }
    // A later verify or an edit has replaced these inputs.
    if (inputs !== this.#verifyInputs) return;
    this.verifyResult = result;
    this.changed();
  }

  /** An input changed, or Clear All: the outcome no longer describes them. */
  clearVerify() {
    this.#verifyInputs += 1;
    if (this.verifyResult === null) return;
    this.verifyResult = null;
    this.changed();
  }

  #signed(inputs: number, result: SignResult, signature = "") {
    if (inputs !== this.#signInputs) {
      this.signResult = { ok: false, text: SignVerifyText.changedWhileSigning };
    } else {
      this.signature = signature;
      this.signResult = result;
    }
    this.changed();
  }
}
