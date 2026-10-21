// Settings ▸ Advanced ▸ Identities (DP6-01, IOS-071): the wallet's identities with the main one
// marked, each identity's keys, Refresh and Refresh all, Find my identities, and the read-only
// look-up of any identity by id or username. Pure view models plus `IdentitiesModel`, which runs
// one action at a time (design note "UI").

import { hasCode, isAppError, type Backend } from "../api/backend";
import type { FoundIdentity } from "../api/generated/FoundIdentity";
import type { IdentityKeyInfo } from "../api/generated/IdentityKeyInfo";
import type { IdentityRefresh } from "../api/generated/IdentityRefresh";
import type { IdentitySummary } from "../api/generated/IdentitySummary";
import type { KeyPurposeName } from "../api/generated/KeyPurposeName";
import type { KeyTypeName } from "../api/generated/KeyTypeName";
import type { SecurityLevelName } from "../api/generated/SecurityLevelName";
import { Model } from "../onboarding/model";
import { GrantModel } from "../tools/grant";
import { headerModel } from "./profile";

export const IDENTITIES_COPY = {
  title: "Identities",
  intro: "The Dash Platform identities of this wallet. The main identity is the one DashPay uses.",
  loading: "Loading identities…",
  readFailed: "Could not read this wallet's identities.",
  tryAgain: "Try again",
  none: "This wallet has no identities.",
  main: "Main",
  index: (n: number) => `Index ${n}`,
  setMain: "Set as main",
  keys: "Keys",
  refresh: "Refresh",
  refreshAll: "Refresh all",
  findMine: "Find my identities",
  lookUp: "Look up",
  unknown: "Unknown",
  updating: "Updating…",
  unverified: "Unverified",
  refreshed: (n: number) => (n === 1 ? "Refreshed 1 identity" : `Refreshed ${n} identities`),
  refreshFailed: (n: number) => (n === 1 ? "1 identity could not be refreshed" : `${n} identities could not be refreshed`),
  refreshedOne: "Identity refreshed",
  found: (n: number) => (n === 1 ? "Found 1 new identity" : `Found ${n} new identities`),
  foundNone: "No new identities found",
  mainSet: "Main identity changed",
  detail: {
    title: "Identity",
    identity: "Identity ID",
    index: "Index",
    revision: "Revision",
    usernames: "Usernames",
    mainName: "Main name",
    setMainName: "Set as main name",
    keys: "Public keys",
    noKeys: "No public keys",
    loading: "Loading keys…",
    close: "Close",
  },
  table: { id: "ID", purpose: "Purpose", level: "Security level", type: "Type", status: "Status", bound: "Bound to" },
  status: { enabled: "Enabled", disabled: "Disabled", readOnly: "Read-only" },
  boundDashPay: "DashPay contact requests",
  find: {
    title: "Look up an identity",
    field: "Identity ID or username",
    submit: "Find",
    invalid: "Enter an identity ID or a username",
    searching: "Searching…",
    notFound: "No identity with this ID or username",
    byId: "Found by identity ID",
    byName: (label: string) => `Found by username @${label}`,
    noUsername: "No username",
    own: "This is one of your identities",
    balance: "Credit balance",
  },
} as const;

/** The copy of a failed identity read or action, by code. */
const BY_CODE: Record<string, string> = {
  "platform.unavailable": "Dash Platform did not answer. Try again.",
  "platform.context_unavailable": "Waiting for the network to sync and verify this identity. Try again shortly.",
  "platform.trust_mismatch": "This identity's data could not be verified against the network.",
  "identity.not_found": "This identity is not on Dash Platform.",
  "platform.timeout": "Dash Platform took too long to answer. Try again.",
  invalid_argument: IDENTITIES_COPY.find.invalid,
};

const codeText = (code: string) => BY_CODE[code] ?? `Something went wrong (${code}).`;

export function identityErrorText(e: unknown): string {
  return isAppError(e) ? codeText(e.code) : "Something went wrong.";
}

export const shortId = (id: string) => (id.length > 12 ? `${id.slice(0, 6)}…${id.slice(-6)}` : id);

/** One row of the list (L4). */
export interface IdentityRow {
  identity: string;
  /** "@name", or the names state's text (DP1-08 D23). */
  username: string;
  /** It owns no name: the row offers DP1-09's credits path (D5). */
  noName: boolean;
  initials: string;
  isMain: boolean;
  index: string;
  /** The credit balance's credits; `null` while unknown. */
  credits: string | null;
  /** The balance may be behind: the row says "Updating…" beside it. */
  stale: boolean;
  unverified: boolean;
}

export function identityRow(i: IdentitySummary): IdentityRow {
  const h = headerModel(i);
  return {
    identity: i.identity,
    username: h.username,
    noName: h.names === "none",
    initials: h.initials,
    isMain: i.isMain,
    index: IDENTITIES_COPY.index(i.index),
    credits: i.balance,
    stale: i.balanceStale,
    unverified: i.unverified,
  };
}

/** The list (L1–L5); rows in index order, as the engine has them. */
export type ListModel = { kind: "loading" } | { kind: "failed" } | { kind: "none" } | { kind: "rows"; rows: IdentityRow[] };

export function listModel(q: { data?: IdentitySummary[]; isError: boolean }): ListModel {
  if (q.data === undefined) return q.isError ? { kind: "failed" } : { kind: "loading" };
  if (q.data.length === 0) return { kind: "none" };
  return { kind: "rows", rows: [...q.data].sort((a, b) => a.index - b.index).map(identityRow) };
}

const PURPOSE: Record<KeyPurposeName, string> = {
  authentication: "Authentication",
  encryption: "Encryption",
  decryption: "Decryption",
  transfer: "Transfer",
  system: "System",
  voting: "Voting",
  owner: "Owner",
};

const LEVEL: Record<SecurityLevelName, string> = { master: "Master", critical: "Critical", high: "High", medium: "Medium" };

const KEY_TYPE: Record<KeyTypeName, string> = {
  ecdsaSecp256k1: "ECDSA secp256k1",
  bls12381: "BLS12-381",
  ecdsaHash160: "ECDSA Hash160",
  bip13ScriptHash: "BIP13 script hash",
  eddsaHash160: "EdDSA Hash160",
};

/** One row of the keys table. */
export interface KeyRow {
  id: number;
  purpose: string;
  level: string;
  type: string;
  status: "enabled" | "disabled" | "readOnly";
  statusText: string;
  /** The contract the key is bound to; `null` when it is not. */
  bound: string | null;
}

export function keyRow(k: IdentityKeyInfo): KeyRow {
  const status = k.disabledAt !== null ? "disabled" : k.readOnly ? "readOnly" : "enabled";
  const bound =
    k.contractBound === null ? null : k.contractBoundDocumentType === "contactRequest" ? IDENTITIES_COPY.boundDashPay : shortId(k.contractBound);
  return {
    id: k.id,
    purpose: PURPOSE[k.purpose],
    level: LEVEL[k.securityLevel],
    type: KEY_TYPE[k.keyType],
    status,
    statusText: IDENTITIES_COPY.status[status],
    bound,
  };
}

export const keyRows = (keys: IdentityKeyInfo[]) => [...keys].sort((a, b) => a.id - b.id).map(keyRow);

/** What the look-up field holds: nothing, something that is neither an identity id nor a
 * username, or the query to send (trimmed). The engine checks it again. */
export type FindInput = { kind: "empty" } | { kind: "invalid" } | { kind: "query"; query: string };

export const BASE58_ID = /^[1-9A-HJ-NP-Za-km-z]{42,44}$/;
const LABEL = /^[A-Za-z0-9](?:[A-Za-z0-9-]{1,61}[A-Za-z0-9])$/;

export function findInput(text: string): FindInput {
  const t = text.trim();
  if (t === "") return { kind: "empty" };
  const label = t.replace(/^@/, "").replace(/\.dash$/i, "");
  if (BASE58_ID.test(t)) return { kind: "query", query: t };
  if (LABEL.test(label)) return { kind: "query", query: label };
  return { kind: "invalid" };
}

/** A found identity as the panel shows it. */
export interface FoundModel {
  identity: string;
  how: string;
  usernames: string;
  credits: string | null;
  revision: string;
  keys: KeyRow[];
  own: boolean;
  unverified: boolean;
}

export function foundModel(f: FoundIdentity): FoundModel {
  const C = IDENTITIES_COPY.find;
  return {
    identity: f.identity,
    how: f.foundBy.kind === "name" ? C.byName(f.foundBy.label) : C.byId,
    usernames: f.names.length > 0 ? f.names.map((n) => `@${n}`).join(", ") : C.noUsername,
    credits: f.balance,
    revision: f.revision === null ? IDENTITIES_COPY.unknown : String(f.revision),
    keys: keyRows(f.publicKeys),
    own: f.own,
    unverified: f.unverified,
  };
}

/** Refresh all's line (D9). */
export function refreshAllText(results: IdentityRefresh[]): { ok: boolean; text: string } {
  const failed = results.filter((r) => r.failureCode !== null);
  if (failed.length === 0) return { ok: true, text: IDENTITIES_COPY.refreshed(results.length) };
  const first = codeText(failed[0].failureCode!);
  return { ok: false, text: `${IDENTITIES_COPY.refreshFailed(failed.length)}: ${first}` };
}

export const discoverText = (n: number) => (n === 0 ? IDENTITIES_COPY.foundNone : IDENTITIES_COPY.found(n));

type Api = Pick<
  Backend,
  | "setMainIdentity"
  | "identityRefresh"
  | "identityRefreshAll"
  | "identityFind"
  | "discoverIdentities"
  | "setMainName"
  | "authorize"
  | "revokeGrant"
  | "lockStatus"
  | "paymentAuthentication"
  | "unlock"
  | "unlockMixingOnly"
>;

/** The page's line after an action: which row it is on (`null`: the header's), and whether it
 * failed. */
export interface ActionResult {
  identity: string | null;
  ok: boolean;
  text: string;
}

export type FindState =
  | { kind: "idle" }
  | { kind: "invalid" }
  | { kind: "searching"; query: string }
  | { kind: "notFound" }
  | { kind: "failed"; text: string; query: string }
  | { kind: "found"; found: FoundModel };

/** The page's actions, one at a time; the list re-reads on the `Platform{Identities}` signal the
 * engine sends after each write (D3), so nothing here holds the list. */
export class IdentitiesModel extends Model {
  /** What runs: `setMain:<id>`, `refresh:<id>`, `refreshAll`, `discover`, `mainName:<id>`. */
  running: string | null = null;
  result: ActionResult | null = null;
  find: FindState = { kind: "idle" };
  readonly grants: GrantModel;
  #findSeq = 0;

  constructor(
    private readonly api: Api,
    readonly walletId: string,
  ) {
    super();
    this.grants = new GrantModel(api);
    this.grants.subscribe(() => this.changed());
  }

  /** Runs `call` unless an action runs; its line is the answer's (`null`: none) or the failure's. */
  async #run(what: string, identity: string | null, call: () => Promise<{ ok: boolean; text: string } | null>): Promise<void> {
    if (this.running !== null) return;
    this.running = what;
    this.result = null;
    this.changed();
    try {
      const r = await call();
      this.result = r && { identity, ...r };
    } catch (e) {
      this.result = { identity, ok: false, text: identityErrorText(e) };
    } finally {
      this.running = null;
      this.changed();
    }
  }

  setMain(identity: string) {
    return this.#run(`setMain:${identity}`, identity, async () => {
      await this.api.setMainIdentity(this.walletId, identity);
      return { ok: true, text: IDENTITIES_COPY.mainSet };
    });
  }

  setMainName(identity: string, label: string) {
    return this.#run(`mainName:${identity}`, identity, async () => {
      await this.api.setMainName(this.walletId, identity, label);
      return null;
    });
  }

  refresh(identity: string) {
    return this.#run(`refresh:${identity}`, identity, async () => {
      await this.api.identityRefresh(this.walletId, identity);
      return { ok: true, text: IDENTITIES_COPY.refreshedOne };
    });
  }

  refreshAll() {
    return this.#run("refreshAll", null, async () => refreshAllText(await this.api.identityRefreshAll(this.walletId)));
  }

  /** Find my identities: same-seed discovery under an identity-scan grant (D5). */
  discover() {
    return this.#run("discover", null, async () => {
      const r = await this.grants.withGrant({ kind: "identityScan" }, this.walletId, (g) => this.api.discoverIdentities(this.walletId, g.id));
      return r === null ? null : { ok: true, text: discoverText(r.value) };
    });
  }

  /** Looks `text` up (read-only, D6); a newer look-up overtakes an older one's answer. */
  async lookUp(text: string) {
    const input = findInput(text);
    if (input.kind !== "query") {
      this.find = input.kind === "invalid" ? { kind: "invalid" } : { kind: "idle" };
      this.changed();
      return;
    }
    const seq = ++this.#findSeq;
    this.find = { kind: "searching", query: input.query };
    this.changed();
    let next: FindState;
    try {
      const found = await this.api.identityFind(this.walletId, input.query);
      next = found === null ? { kind: "notFound" } : { kind: "found", found: foundModel(found) };
    } catch (e) {
      next = hasCode(e, "invalid_argument") ? { kind: "invalid" } : { kind: "failed", text: identityErrorText(e), query: input.query };
    }
    if (seq !== this.#findSeq) return;
    this.find = next;
    this.changed();
  }

  clearFind() {
    this.#findSeq++;
    this.find = { kind: "idle" };
    this.changed();
  }
}

const models = new Map<string, IdentitiesModel>();

/** The page's model for `walletId`, kept across visits so a running action keeps its line. */
export function identitiesFor(api: Api, walletId: string): IdentitiesModel {
  let m = models.get(walletId);
  if (!m) {
    m = new IdentitiesModel(api, walletId);
    models.set(walletId, m);
  }
  return m;
}
