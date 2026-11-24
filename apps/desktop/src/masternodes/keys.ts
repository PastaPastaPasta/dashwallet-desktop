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
import { isAppError } from "../api/backend";
import { failureText, Model } from "../onboarding/model";
import { mnErrorText } from "./model";

export const KEY_ROLES: readonly MasternodeKeyRoleName[] = ["owner", "voting", "operator", "platform_node"];

/** Operator and platform node secrets go to the server that runs the node, so they copy; owner
 * and voting secrets never leave the wallet and refuse copying, as the recovery phrase does. */
export const canCopySecret = (role: MasternodeKeyRoleName) => role === "operator" || role === "platform_node";

/** Keys per page, for every role (MG-10 §1.5). */
export const KEYS_PAGE = 20;

/** A known masternode a key matched, as its row reads. */
export interface KeyMatch {
  proTxHash: string;
  name: string;
  service: string | null;
}

/** Base58Check addresses are case-sensitive; BLS keys and node ids are hex, of either case. */
const sameAddress = (a: string | null | undefined, b: string | null | undefined) => !!a && !!b && a === b;
const sameHex = (a: string | null | undefined, b: string | null | undefined) => !!a && !!b && a.toLowerCase() === b.toLowerCase();

/** The known masternodes whose registration or list entry carries `key` (D8). */
export function keyMatches(key: MasternodeKeyInfo, details: readonly MasternodeDetail[], name: (d: MasternodeDetail) => string): KeyMatch[] {
  return details
    .filter((d) => {
      switch (key.role) {
        case "owner":
          return sameAddress(key.address, d.registration?.ownerAddress);
        case "voting":
          // The live list wins: after a key change the registration record still names the old one.
          return sameAddress(key.address, d.list ? d.list.votingAddress : d.registration?.votingAddress);
        case "operator":
          return sameHex(key.publicKeyHex, d.list?.operatorPublicKey) || sameHex(key.legacyPublicKeyHex, d.list?.operatorPublicKey);
        case "platform_node":
          return sameHex(key.platformNodeId, d.list?.platformNodeId);
      }
    })
    .map((d) => ({ proTxHash: d.row.proTxHash, name: name(d), service: d.list?.service ?? d.row.service }));
}

/** A query's state, as `useQuery` reports it. */
export interface ReadState<T> {
  data: T | undefined;
  isError: boolean;
  isPending: boolean;
  isFetching: boolean;
  /** Waiting to run: a refresh the query client holds while the window reports itself offline. */
  isPaused: boolean;
}

export type MatchState = "checking" | "failed" | "done";

/** The known masternodes keys match against, and whether that is settled (D8). Matches come only
 * once every read has settled with success: a refresh in flight, or one held while offline, is "checking", and a failed one,
 * whose old data stays cached, is "failed". */
export function knownMatches<L, D>(list: ReadState<L>, details: readonly ReadState<D>[]): { details: D[]; state: MatchState } {
  const reads = [list, ...details];
  // A retry after a failure reads as checking until it settles (with cached data it keeps its error).
  if (reads.some((r) => r.isPending || r.isFetching || r.isPaused)) return { details: [], state: "checking" };
  if (reads.some((r) => r.isError)) return { details: [], state: "failed" };
  return { details: details.map((d) => d.data!), state: "done" };
}

type Api = Pick<Backend, "lockStatus" | "unlock" | "unlockMixingOnly" | "authorize" | "mnRevealKey" | "revokeGrant">;

export type RevealStep = "loading" | "failed" | "hidden" | "working" | "shown";

/** A keys call's error as a catalogued sentence: the masternodes table, else the shared one. */
export function keysErrorText(e: unknown): string {
  if (!isAppError(e)) return failureText(e);
  if (e.code.startsWith("masternodes.")) return mnErrorText(e);
  // The keychain's own vault codes read as the vault's.
  if (e.code === "masternode.grant_invalid") return failureText({ ...e, code: "vault.grant_invalid" });
  if (e.code === "masternode.vault_locked") return failureText({ ...e, code: "vault.locked" });
  return failureText(e);
}

/** The one keypair whose secret may show. Dropped on hide, on another keypair, and on dispose. */
export class KeyRevealModel extends Model {
  step: RevealStep = "loading";
  /** A passphrase is needed (encrypted vault). */
  encrypted = true;
  readonly lock: LockModel;
  error: string | null = null;
  secret: RevealedMasternodeKey | null = null;
  #epoch = 0;
  /** A grant issued and not yet known to be redeemed: dispose revokes it. */
  #grant: string | null = null;

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
    // Without the vault's state and its throttle, Reveal stays off (the lock policy is unknown).
    try {
      this.encrypted = (await this.api.lockStatus()).encrypted;
      if (this.encrypted) await this.lock.load();
    } catch (e) {
      this.error = keysErrorText(e);
      return this.#go("failed");
    }
    // The lock model keeps its own read's failure as a message rather than throwing.
    if (this.encrypted && this.lock.lockState === null) {
      this.error = this.lock.message ?? keysErrorText(null);
      return this.#go("failed");
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
          // A reveal that Hide dropped leaves a newer one's state alone.
          if (epoch === this.#epoch) this.error = keysErrorText(e);
          return false;
        },
      );
      if (!authorized && epoch === this.#epoch) this.#go("hidden");
    }
    if (!authorized || !grant) return this.changed();
    if (epoch !== this.#epoch) return this.#revoke(grant);
    this.#grant = grant;
    this.#go("working");
    try {
      const secret = await this.api.mnRevealKey(this.walletId, this.role, this.index, grant);
      // A newer reveal may hold the slot now; only this reveal's own grant is cleared.
      if (this.#grant === grant) this.#grant = null;
      if (epoch !== this.#epoch) return;
      this.secret = secret;
      this.#go("shown");
    } catch (e) {
      if (this.#grant === grant) this.#grant = null;
      this.#revoke(grant);
      if (epoch !== this.#epoch) return;
      this.error = keysErrorText(e);
      this.#go("hidden");
    }
  }

  hide() {
    this.#epoch++;
    this.secret = null;
    // A reveal still in flight is dropped, and its grant with it.
    if (this.#grant) this.#revoke(this.#grant);
    this.#grant = null;
    // A failed load stays failed: hiding never turns Reveal on.
    if (this.step !== "loading" && this.step !== "failed") this.#go("hidden");
  }

  /** The keypair closed, the sheet closed or the wallet changed: a reveal in flight is dropped. */
  dispose() {
    this.#epoch++;
    this.secret = null;
    // A reveal still in flight: its grant is revoked now, whether or not the call redeems it first.
    if (this.#grant) this.#revoke(this.#grant);
    this.#grant = null;
  }

  #revoke(grant: string) {
    this.api.revokeGrant(grant).catch(() => {});
  }

  #go(step: RevealStep) {
    this.step = step;
    this.changed();
  }
}
