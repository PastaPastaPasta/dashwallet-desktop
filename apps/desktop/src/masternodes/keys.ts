// Masternode keys (IOS-083, MG-10 §1.5, MG-00 D7): the keychain's keys per role, each matched to
// the known masternodes that carry it (D8: owner and voting by address, operator by either BLS
// form, platform node by node id; never "used"), and one secret revealed at a time behind a
// single-use `revealSecret` grant, through the lock screen's policy as Show Recovery Phrase does.

import type { Backend } from "../api/backend";
import type { MasternodeDetail } from "../api/generated/MasternodeDetail";
import type { MasternodeKeyInfo } from "../api/generated/MasternodeKeyInfo";
import type { MasternodeKeyRoleName } from "../api/generated/MasternodeKeyRoleName";
import type { RevealedMasternodeKey } from "../api/generated/RevealedMasternodeKey";
import { LockModel } from "../onboarding/lock";
import { Model } from "../onboarding/model";
import { mnErrorText } from "./model";

export const KEY_ROLES: readonly MasternodeKeyRoleName[] = ["owner", "voting", "operator", "platform_node"];

/** Keys per page, for every role (MG-10 §1.5). */
export const KEYS_PAGE = 20;

/** A known masternode a key matched, as its row reads. */
export interface KeyMatch {
  proTxHash: string;
  name: string;
  service: string | null;
}

const same = (a: string | null | undefined, b: string | null | undefined) => !!a && !!b && a.toLowerCase() === b.toLowerCase();

/** The known masternodes whose registration or list entry carries `key` (D8). */
export function keyMatches(key: MasternodeKeyInfo, details: readonly MasternodeDetail[], name: (d: MasternodeDetail) => string): KeyMatch[] {
  return details
    .filter((d) => {
      switch (key.role) {
        case "owner":
          return same(key.address, d.registration?.ownerAddress);
        case "voting":
          return same(key.address, d.list?.votingAddress) || same(key.address, d.registration?.votingAddress);
        case "operator":
          return same(key.publicKeyHex, d.list?.operatorPublicKey) || same(key.legacyPublicKeyHex, d.list?.operatorPublicKey);
        case "platform_node":
          return same(key.platformNodeId, d.list?.platformNodeId);
      }
    })
    .map((d) => ({ proTxHash: d.row.proTxHash, name: name(d), service: d.list?.service ?? d.row.service }));
}

type Api = Pick<Backend, "lockStatus" | "unlock" | "unlockMixingOnly" | "authorize" | "mnRevealKey" | "revokeGrant">;

export type RevealStep = "loading" | "hidden" | "working" | "shown";

/** The one keypair whose secret may show. Dropped on hide, on another keypair, and on dispose. */
export class KeyRevealModel extends Model {
  step: RevealStep = "loading";
  /** A passphrase is needed (encrypted vault). */
  encrypted = true;
  readonly lock: LockModel;
  error: string | null = null;
  secret: RevealedMasternodeKey | null = null;
  #epoch = 0;

  constructor(
    private readonly api: Api,
    readonly walletId: string,
    readonly role: MasternodeKeyRoleName,
    readonly index: number,
  ) {
    super();
    this.lock = new LockModel(api);
  }

  async load() {
    try {
      this.encrypted = (await this.api.lockStatus()).encrypted;
      if (this.encrypted) await this.lock.load();
    } catch (e) {
      this.error = mnErrorText(e);
    }
    this.#go("hidden");
  }

  /** Checks the passphrase (ignored on an unencrypted vault) and shows the secret. */
  async reveal(passphrase: string) {
    if (this.step !== "hidden" || this.lock.working) return;
    const epoch = this.#epoch;
    this.error = null;
    let grant: string | null = null;
    const authorize = async (p: string) => {
      const credential = this.encrypted ? ({ kind: "passphrase", passphrase: p } as const) : ({ kind: "unencrypted" } as const);
      grant = (await this.api.authorize({ kind: "revealSecret" }, this.walletId, credential)).id;
    };
    let authorized: boolean;
    if (this.encrypted) authorized = await this.lock.attempt(passphrase, authorize);
    else {
      this.#go("working");
      authorized = await authorize("").then(
        () => true,
        (e) => {
          this.error = mnErrorText(e);
          return false;
        },
      );
      if (!authorized) this.#go("hidden");
    }
    if (!authorized || !grant) return this.changed();
    if (epoch !== this.#epoch) return this.#revoke(grant);
    this.#go("working");
    try {
      const secret = await this.api.mnRevealKey(this.walletId, this.role, this.index, grant);
      if (epoch !== this.#epoch) return;
      this.secret = secret;
      this.#go("shown");
    } catch (e) {
      this.#revoke(grant);
      if (epoch !== this.#epoch) return;
      this.error = mnErrorText(e);
      this.#go("hidden");
    }
  }

  hide() {
    this.#epoch++;
    this.secret = null;
    if (this.step !== "loading") this.#go("hidden");
  }

  /** The keypair closed, the sheet closed or the wallet changed: a reveal in flight is dropped. */
  dispose() {
    this.#epoch++;
    this.secret = null;
  }

  #revoke(grant: string) {
    this.api.revokeGrant(grant).catch(() => {});
  }

  #go(step: RevealStep) {
    this.step = step;
    this.changed();
  }
}
