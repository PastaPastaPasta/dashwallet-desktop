// Tools ▸ Sign/Verify (QT-099, QT-100): the Swift `SignVerifyViewModel`, ported. Sign checks the
// address first with dash-qt's texts, then asks for a `signMessage` grant (U-05 D15); a grant the
// signature did not use is revoked. Verify shows the engine's outcome and dash-qt's text.

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
    this.signature = "";
    this.signResult = null;
    this.changed();
    try {
      const kind = await this.api.classifyAddress(this.network, a).catch(() => null);
      if (kind?.kind !== "core") return this.#signed({ ok: false, text: SignVerifyText.invalidAddress });
      if (kind.scriptHash) return this.#signed({ ok: false, text: SignVerifyText.addressNoKey });
      let grant;
      try {
        grant = await this.grants.obtain({ kind: "signMessage" }, this.walletId, since);
      } catch (e) {
        return this.#signed({ ok: false, text: failureText(e) });
      }
      if (grant === null) return this.#signed({ ok: false, text: SignVerifyText.unlockCancelled });
      try {
        this.signature = await this.api.signMessage(this.walletId, a, message, grant.id);
        this.#signed({ ok: true, text: SignVerifyText.signed });
      } catch (e) {
        this.grants.revoke(grant);
        this.#signed({ ok: false, text: signErrorText(e) });
      }
    } finally {
      this.signing = false;
      this.changed();
    }
  }

  clearSign() {
    this.signature = "";
    this.signResult = null;
    this.changed();
  }

  async verify(address: string, signature: string, message: string) {
    try {
      this.verifyResult = await this.api.verifyMessage(address.trim(), signature.trim(), message);
    } catch (e) {
      this.verifyResult = { outcome: "invalidAddress", text: failureText(e) };
    }
    this.changed();
  }

  clearVerify() {
    this.verifyResult = null;
    this.changed();
  }

  #signed(result: SignResult) {
    this.signResult = result;
    this.changed();
  }
}
