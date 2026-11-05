// The grant a tool asks for before the engine spends, signs or reveals (U-05 D7, D15, D17, D12):
// the vault's own credential when no passphrase is needed, else a passphrase prompt that goes
// through `LockModel.attempt`, so every try counts and waits as on the lock screen (U-02 D12).
// One request at a time; the screen shows the prompt while `pending` is set.

import { hasCode, type Backend } from "../api/backend";
import type { AuthGrant } from "../api/generated/AuthGrant";
import type { CredentialSpec } from "../api/generated/CredentialSpec";
import type { GrantPurposeSpec } from "../api/generated/GrantPurposeSpec";
import type { LockStatus } from "../api/generated/LockStatus";
import { LockModel } from "../onboarding/lock";
import { Model } from "../onboarding/model";

/** `authorizePlatformSend` only where a tool asks for a `platformSend` grant (Pay from Platform). */
type Api = Pick<Backend, "authorize" | "revokeGrant" | "lockStatus" | "paymentAuthentication" | "unlock" | "unlockMixingOnly"> & Partial<Pick<Backend, "authorizePlatformSend">>;

/** What a grant is asked for: a vault purpose, or the quoted Platform send `operation`, whose
 * `platformOp` grant the engine sizes from the quote and binds to that operation alone. */
export type GrantAsk = GrantPurposeSpec | { kind: "platformSend"; operation: string };

/** Payments and Platform spends follow "Require authentication for every payment" (E0-13 D11);
 * the rest need the passphrase only while the vault is not fully unlocked. */
export function needsPassphrase(purpose: GrantAsk, lock: LockStatus, paymentAuth: boolean): boolean {
  if (!lock.encrypted) return false;
  if (purpose.kind === "spend" || purpose.kind === "platformOp" || purpose.kind === "platformSend") return paymentAuth || lock.state !== "unlocked";
  return lock.state !== "unlocked";
}

/** What the passphrase prompt says for a grant that moves money (a shield, a Shielded or Platform
 * move, a withdrawal, a top-up): what a user thinks they are doing (DEC-1058 #8). */
export const PAYMENT_PROMPT = "Enter your passphrase to authorize this payment.";

export interface GrantRequest {
  purpose: GrantAsk;
  walletId: string | null;
}

export class GrantModel extends Model {
  /** The request waiting for the passphrase; `null` while none is. */
  pending: GrantRequest | null = null;
  /** What the prompt says instead of its purpose's own words. */
  prompt: string | null = null;
  readonly lock: LockModel;
  #answer: ((grant: AuthGrant | null) => void) | null = null;
  /** Set from the first moment of `obtain` to its end, so two requests never overlap. */
  #asking = false;
  /** Moves on every `cancel`: a request cancelled before its prompt showed ends there. */
  #epoch = 0;

  /** Taken when a tool's action begins and passed to `obtain` as `since`: a cancel after the
   * action began (a tab or wallet switch) then ends a request it makes later. */
  get epoch(): number {
    return this.#epoch;
  }

  constructor(private readonly api: Api) {
    super();
    this.lock = new LockModel(api);
    this.lock.subscribe(() => this.changed());
  }

  /** A grant for `purpose`: at once when the vault's credential will do, else once the user
   * entered the passphrase; `null` when they cancelled, or when a cancel came after `since`
   * (the action's `epoch`). Other refusals throw. */
  async obtain(purpose: GrantAsk, walletId: string | null, since = this.#epoch): Promise<AuthGrant | null> {
    if (since !== this.#epoch) return null;
    if (this.#asking) throw new Error("a grant is already being asked for");
    this.#asking = true;
    const epoch = since;
    try {
      const [lock, paymentAuth] = await Promise.all([this.api.lockStatus(), this.api.paymentAuthentication().catch(() => true)]);
      if (epoch !== this.#epoch) return null;
      if (!needsPassphrase(purpose, lock, paymentAuth)) {
        try {
          const grant = await this.#issue(purpose, walletId, { kind: "unencrypted" });
          if (epoch === this.#epoch) return grant;
          this.revoke(grant);
          return null;
        } catch (e) {
          // The vault locked meanwhile, or the policy says otherwise: ask after all.
          if (!hasCode(e, "vault.credential_required") && !hasCode(e, "vault.locked") && !hasCode(e, "vault.mixing_only")) throw e;
        }
        if (epoch !== this.#epoch) return null;
      }
      const answer = new Promise<AuthGrant | null>((resolve) => (this.#answer = resolve));
      this.pending = { purpose, walletId };
      this.changed();
      void this.lock.load();
      return await answer;
    } finally {
      this.#asking = false;
    }
  }

  /** One passphrase attempt for the pending request; `true` once it was granted. */
  async submit(passphrase: string): Promise<boolean> {
    const request = this.pending;
    if (!request) return false;
    let grant: AuthGrant | null = null;
    const ok = await this.lock.attempt(passphrase, async (p) => {
      grant = await this.#issue(request.purpose, request.walletId, { kind: "passphrase", passphrase: p });
    });
    if (!grant) return ok;
    // Cancelled (or another request began) while the passphrase was checked: this grant has
    // no taker, so it is dropped, never handed to the next request.
    if (this.pending !== request) {
      this.revoke(grant);
      return false;
    }
    if (ok) this.#finish(grant);
    return ok;
  }

  cancel() {
    this.#epoch++;
    if (this.pending) this.#finish(null);
  }

  /** `run` under a grant for `purpose`; a grant `run` did not use (it threw) is revoked. `null`
   * when the request was cancelled (`since` as for `obtain`); a refusal throws. */
  async withGrant<T>(
    purpose: GrantAsk,
    walletId: string | null,
    run: (grant: AuthGrant) => Promise<T>,
    since = this.#epoch,
  ): Promise<{ value: T } | null> {
    const grant = await this.obtain(purpose, walletId, since);
    if (grant === null) return null;
    try {
      return { value: await run(grant) };
    } catch (e) {
      this.revoke(grant);
      throw e;
    }
  }

  #issue(ask: GrantAsk, walletId: string | null, credential: CredentialSpec): Promise<AuthGrant> {
    if (ask.kind !== "platformSend") return this.api.authorize(ask, walletId, credential);
    if (walletId === null || !this.api.authorizePlatformSend) throw new Error("no send grant here");
    return this.api.authorizePlatformSend(walletId, ask.operation, credential);
  }

  /** Drops a grant the call did not use (unknown or spent ids are ignored). */
  revoke(grant: AuthGrant) {
    void this.api.revokeGrant(grant.id).catch(() => null);
  }

  #finish(grant: AuthGrant | null) {
    const answer = this.#answer;
    this.#answer = null;
    this.pending = null;
    this.lock.message = null;
    this.changed();
    answer?.(grant);
  }
}
