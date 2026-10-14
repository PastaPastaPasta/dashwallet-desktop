// The lock screen (IOS-012, IOS-013, QT-111, QT-112): the Swift `LockViewModel`, ported (U-02 D8).
// The vault throttles by itself (dw-vault `throttle_wait_secs`: 6^(n−3) minutes from the third
// failure) and stores the failure count in its file; the policy adds the count of attempts left
// and the cut-off at eight, after which only Forgot passphrase is offered.

import type { Backend } from "../api/backend";
import { hasCode, retryAfterSecs } from "../api/backend";
import type { LockStateName } from "../api/generated/LockStateName";
import type { LockStatus } from "../api/generated/LockStatus";
import { failureText, Model, PASSPHRASE_REQUIRED, tryAgainIn, WRONG_PASSPHRASE } from "./model";

export type LockVerdict = { kind: "allowed" } | { kind: "wait"; secs: number } | { kind: "disabled" };

export const LockoutPolicy = {
  freeAttempts: 3,
  disabledAfter: 8,
  /** Three failures are free; after `n ≥ 3` the next attempt waits `6^(n−3)` minutes; at
   * eight passphrase entry stops. */
  verdict(failures: number): LockVerdict {
    if (failures >= this.disabledAfter) return { kind: "disabled" };
    if (failures < this.freeAttempts) return { kind: "allowed" };
    return { kind: "wait", secs: 60 * 6 ** (failures - this.freeAttempts) };
  },
  attemptsRemaining(failures: number): number {
    return Math.max(0, this.disabledAfter - failures);
  },
};

export const LockText = {
  title: "Unlock wallet",
  prompt: "This operation needs your wallet passphrase to unlock the wallet.",
  disabled: "Too many incorrect attempts. Restore the wallet with your recovery phrase to set a new passphrase.",
  attemptsRemaining: (n: number) => (n === 1 ? "1 attempt remaining" : `${n} attempts remaining`),
};

type Api = Pick<Backend, "lockStatus" | "unlock" | "unlockMixingOnly">;

export class LockModel extends Model {
  lockState: LockStateName | null = null;
  failedAttempts = 0;
  /** When the next attempt is accepted (ms since the epoch); `null` when it is now. */
  retryAt: number | null = null;
  /** Eight failures: only Forgot passphrase is offered. */
  disabled = false;
  message: string | null = null;
  working = false;
  /** The vault holds secrets of at least one wallet, so Forgot passphrase can recover it. */
  canRecover = false;

  constructor(
    private readonly api: Api,
    private readonly now: () => number = Date.now,
  ) {
    super();
  }

  /** Seconds until the next attempt; `null` when it is accepted now. */
  get retryAfterSecs(): number | null {
    if (this.retryAt === null) return null;
    const left = Math.ceil((this.retryAt - this.now()) / 1000);
    return left > 0 ? left : null;
  }

  get locked(): boolean {
    return this.lockState === "locked" || this.lockState === "unlockedMixingOnly";
  }

  /** Reads the vault's count and wait, so a restart keeps both. */
  async load() {
    try {
      this.#apply(await this.api.lockStatus());
    } catch (e) {
      this.message = failureText(e);
      this.changed();
    }
  }

  /** Clears a wait that has run out; the screen calls it on a timer. */
  refresh() {
    if (this.retryAt === null || this.retryAfterSecs !== null) return;
    this.retryAt = null;
    if (!this.disabled) this.message = null;
    this.changed();
  }

  /** Unlocks fully, or for mixing only. Refused while waiting or disabled; `true` on success. */
  async unlock(passphrase: string, mixingOnly: boolean): Promise<boolean> {
    if (this.working) return false;
    this.refresh();
    if (this.disabled) return this.#say(LockText.disabled);
    const wait = this.retryAfterSecs;
    if (wait !== null) return this.#say(tryAgainIn(wait));
    if (!passphrase) return this.#say(PASSPHRASE_REQUIRED);
    this.working = true;
    this.changed();
    try {
      const status = await (mixingOnly ? this.api.unlockMixingOnly(passphrase) : this.api.unlock(passphrase));
      this.message = null;
      this.#apply(status);
      return true;
    } catch (e) {
      if (hasCode(e, "vault.wrong_passphrase")) {
        const failures = e.data?.kind === "wrongPassphrase" ? e.data.failedAttempts : this.failedAttempts + 1;
        this.#failed(failures, retryAfterSecs(e));
      } else if (hasCode(e, "vault.throttled")) {
        this.#wait(retryAfterSecs(e));
        this.message = tryAgainIn(retryAfterSecs(e));
      } else {
        this.message = failureText(e);
      }
      return false;
    } finally {
      this.working = false;
      this.changed();
    }
  }

  #apply(status: LockStatus) {
    this.lockState = status.state;
    this.failedAttempts = status.failedAttempts;
    this.canRecover = status.encrypted && status.walletsWithSecrets.length > 0;
    this.disabled = LockoutPolicy.verdict(this.failedAttempts).kind === "disabled";
    // The vault's wait counts from its last failure, so it is the one to trust here.
    this.#wait(status.retryAfterSecs ?? 0);
    if (this.disabled) this.message = LockText.disabled;
    else if (this.retryAt !== null) this.message = tryAgainIn(this.retryAfterSecs ?? 0);
    this.changed();
  }

  #failed(failures: number, vaultWait: number) {
    this.failedAttempts = failures;
    const verdict = LockoutPolicy.verdict(failures);
    this.disabled = verdict.kind === "disabled";
    if (this.disabled) {
      this.retryAt = null;
      this.message = LockText.disabled;
      return;
    }
    const wait = Math.max(verdict.kind === "wait" ? verdict.secs : 0, vaultWait);
    this.#wait(wait);
    this.message =
      wait > 0
        ? `${WRONG_PASSPHRASE} ${tryAgainIn(wait)}`
        : `${WRONG_PASSPHRASE} ${LockText.attemptsRemaining(LockoutPolicy.attemptsRemaining(failures))}`;
  }

  #wait(secs: number) {
    this.retryAt = secs > 0 ? this.now() + secs * 1000 : null;
  }

  #say(message: string): false {
    this.message = message;
    this.changed();
    return false;
  }
}
