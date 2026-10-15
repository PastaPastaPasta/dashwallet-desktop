// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The fixture engine: dw-app's commands over a `World`. Each handler takes the command's
// generated `CommandArgs` and returns its `CommandResults`, so a binding change the fixture
// does not follow fails `tsc`. Mutating commands emit the `AppEvent`s the engine would.
// A command without a handler rejects with `platform.not_implemented` (installFixture).
//
// Grants and leases follow dw-vault and the engine's lease table as the accept vector records
// them (vectors/accept.json, from dw-engine's `accept_grant_vectors`): a lock, and an unlock
// that installs the data key (from locked or mixing-only), revoke every vault grant; a
// passphrase or quick-unlock grant issued while locked carries its own key, so it redeems while
// locked; a lock revokes every live lease (each keeps its `ownKey` flag); an unlock that
// installs the key ends the vault's epoch, so every live lease needs a grant again, its key
// dropped; a vault-grant write runs under a one-shot lease that `leases` lists, ended. Not
// modelled: grant expiry and lease idle reaping (the clock is FIXTURE_NOW), quick unlock's
// spending limit and passphrase age, held identity keys (`identity.keys_missing`), and the lock report, which is the scenario's
// (`World.cut`), not derived from the leases.

import type { AppError, AppEvent, CommandArgs, CommandName, CommandResults } from "../api/generated/commands";
import type { AuthGrant } from "../api/generated/AuthGrant";
import type { Budget } from "../api/generated/Budget";
import type { ContactDetailInfo } from "../api/generated/ContactDetailInfo";
import type { CredentialSpec } from "../api/generated/CredentialSpec";
import type { FlowKindName } from "../api/generated/FlowKindName";
import type { GrantPurposeSpec } from "../api/generated/GrantPurposeSpec";
import type { Lease } from "../api/generated/Lease";
import type { LockOutcome } from "../api/generated/LockOutcome";
import { isCheckable } from "../api/lockReport";
import type { DashPayEventPage } from "../api/generated/DashPayEventPage";
import type { CostTable } from "../api/generated/CostTable";
import type { GrantAction } from "../api/generated/GrantAction";
import type { PrivateContactDetails } from "../api/generated/PrivateContactDetails";
import type { EligibilityName } from "../api/generated/EligibilityName";
import type { UserHit } from "../api/generated/UserHit";
import type { ScannedContactInfo } from "../api/generated/ScannedContactInfo";
import { activityPage, contactList, eventOf, relationOf, type DashPayWorld, type FixtureScan } from "./dashpay";
import { scanHandlers } from "./scan";
import { routeLink } from "./links";
import { normalizePhrase } from "../onboarding/words";
import { sendHandlers } from "./send";
import { toolsHandlers } from "./tools";
import { isClosed, settingsHandlers } from "./settings";
import { emptyWallet, MAX_REPORT_WALLETS, type FixtureGrant, type World } from "./scenarios";
import { checkPhrase, CREATED_12, CREATED_24, throttleWaitSecs, walletIdOf } from "./vault";
import { BASE58, FIXTURE_NOW, historyPage, secs, txDetails } from "./wallet";

export interface Ctx {
  world: World;
  emit(event: AppEvent): void;
}

export type Handlers = { [K in CommandName]?: (args: CommandArgs[K], ctx: Ctx) => CommandResults[K] };

const fail = (code: AppError["code"], message: string, data: AppError["data"] = null): AppError => ({ code, message, data });

function wallet(w: World, walletId: string) {
  const found = w.wallets.find((x) => x.summary.walletId === walletId);
  if (!found || isClosed(w, walletId)) throw fail("wallet_not_found", `wallet not found: ${walletId}`);
  return found;
}

/** The wallet's DashPay state, for one of its identities. */
function dashpay(w: World, walletId: string, identity?: string) {
  wallet(w, walletId);
  const dp = w.dashpayOf?.[walletId] ?? w.dashpay;
  if (identity !== undefined && !dp.identities.some((i) => i.identity === identity))
    throw fail("identity.not_found", `identity not found: ${identity}`);
  return dp;
}

function contact(w: World, walletId: string, identity: string, id: string): ContactDetailInfo {
  const c = dashpay(w, walletId, identity).contacts.find((x) => x.summary.contact === id);
  if (!c) throw fail("invalid_argument", `not a contact: ${id}`);
  return c;
}

function contactsChanged({ world, emit }: Ctx, walletId: string, identity: string) {
  emit({ kind: "platform", network: world.launch.network.dirName, walletId, change: { domain: "contacts", identity } });
}

function setLock({ world, emit }: Ctx, state: World["lock"]["state"]) {
  world.lock = { ...world.lock, state };
  emit({ kind: "vaultLockState", network: world.launch.network.dirName, state });
}

// Grants and leases (E0-04), as dw-vault and the engine's lease table answer them.

/** The quote of a contact request or its acceptance (dw-engine flows.rs `CONTACT_REQUEST_CREDITS`). */
const CONTACT_REQUEST_CREDITS = 150_000_000n;
/** requests.rs: an auto-accept claim needs this long left when its request goes out. */
const SCAN_MARGIN_SECS = 60;
/** flows.rs `DOCUMENT_10K_CREDITS` (a profile or `contactInfo`) and `KEY_UPDATE_CREDITS`. */
const DOCUMENT_10K_CREDITS = 300_000_000n;
const KEY_UPDATE_CREDITS = 100_000_000n;

/** `grant_request`'s bounds (flows.rs): no duffs, the action's credits. */
const GRANT_CREDITS: Record<Exclude<GrantAction["kind"], "registerName">, bigint> = {
  sendRequest: CONTACT_REQUEST_CREDITS,
  acceptRequest: CONTACT_REQUEST_CREDITS,
  updateProfile: DOCUMENT_10K_CREDITS,
  publishPrivateDetails: DOCUMENT_10K_CREDITS,
  enableDashPayKeys: KEY_UPDATE_CREDITS,
};

/** A testnet cost table (DP1-06's shape): the T2 charges measured on 2026-10-11 (m4 item 41). */
const COST_TABLE: CostTable = {
  protocolVersion: 10,
  contactRequest: "94048500",
  profileUpdate: "120000000",
  contactInfo: "110000000",
  enableDashpayKeys: "61330100",
  creditsPerDuff: 1000,
  topUpFee: "50000000",
  topUpMinDuffs: 50_000,
  lowCredits: "1200000000",
  emptyCredits: "61330100",
};

/** DIP-15's `contactInfo` bound on the encrypted alias and note (m4 §5 `details_too_long`). The
 * fixture measures the two as JSON, not DIP-15's exact encoding. */
const DETAILS_MAX_BYTES = 2031;
const GRANT_TTL_SECS = 120;
/** How long a lease holds an own key (the engine's `key_ttl`, as the scenarios' leases show it). */
const KEY_TTL_SECS = 240;

const grantInvalid = () => fail("platform.grant_invalid", "grant invalid");
const grantExceeded = (needed: bigint, remaining: bigint) =>
  fail("platform.grant_exceeded", "grant exceeded", { kind: "budget", purpose: "credits", needed: String(needed), remaining: String(remaining) });

/** Not ended or revoked: a lock revokes it, an epoch change makes it need a grant. */
const live = (l: Lease) => l.state.kind !== "ended" && l.state.kind !== "revoked";

/** No data key in memory: dw-vault's `Locked`, `MixingOnly` or `NoVault`. */
const keyless = (w: World) => w.lock.state === "locked" || w.lock.state === "unlockedMixingOnly" || w.lock.state === "noVault";

/** A fresh 32-hex-digit id whose first 8 digits are unique (a lease's `Lease.id`). */
function freshId(w: World, tag: string): string {
  w.issued += 1;
  return (tag + w.issued.toString(16).padStart(4, "0")).repeat(4);
}

/** `parse_lease`: `lease-` and 32 hex digits, else a vault grant id. */
function leaseKey(id: string): string | null {
  return /^lease-[0-9a-f]{32}$/i.test(id) ? id.toLowerCase() : null;
}

/** dw-vault's credential check of `authorize`. While "Require authentication for every payment"
 * is on (E0-13 D11), a `spend` grant of an encrypted vault needs a real credential: the
 * unlocked vault's own (`unencrypted`) is `vault.credential_required`. */
function checkCredential(w: World, credential: CredentialSpec, purposes: GrantPurposeSpec[]) {
  if (w.lock.state === "noVault") throw fail("vault.no_vault", "no vault");
  // E0-13 D11 (`check_payment_auth`): a locked vault answers `vault.locked` below instead.
  const pays = purposes.some((p) => p.kind === "spend" || p.kind === "platformOp");
  if (credential.kind === "unencrypted" && w.lock.encrypted && w.lock.state !== "locked" && w.paymentAuth && pays)
    throw fail("vault.credential_required", "every payment needs authentication");
  switch (credential.kind) {
    case "passphrase": {
      if (credential.passphrase !== w.passphrase) {
        // As `unlock` counts a wrong passphrase.
        const failedAttempts = w.lock.failedAttempts + 1;
        w.lock = { ...w.lock, failedAttempts };
        if (w.throttleSecs) w.throttledUntil = Date.now() + w.throttleSecs * 1000;
        throw fail("vault.wrong_passphrase", `wrong passphrase (${failedAttempts} failed)`, {
          kind: "wrongPassphrase",
          failedAttempts,
          retryAfterSecs: w.throttleSecs,
        });
      }
      w.lock = { ...w.lock, failedAttempts: 0 };
      return;
    }
    case "quickUnlock":
      if (!w.lock.quickUnlockEnrolled) throw fail("vault.quick_unlock_unavailable", "quick unlock is not enrolled");
      return;
    case "unencrypted":
      if (w.lock.state === "locked") throw fail("vault.locked", "vault locked");
      if (w.lock.state === "unlockedMixingOnly") throw fail("vault.mixing_only", "vault unlocked for mixing only");
      return;
  }
}

/** `Vault::authorize_set`: one credential check, one single-use grant per purpose, all or none.
 * A grant issued while the vault holds no data key carries its own. */
function authorizeSet(w: World, purposes: GrantPurposeSpec[], walletId: string | null, credential: CredentialSpec): AuthGrant[] {
  if (walletId !== null && !/^[0-9a-f]{64}$/i.test(walletId)) throw fail("invalid_argument", `not a wallet id: ${walletId}`);
  if (purposes.length === 0) throw fail("invalid_argument", "no grant purpose");
  const unbound = purposes.find((p) => (p.kind !== "changeCredential") !== (walletId !== null));
  if (unbound) throw fail("invalid_argument", `a ${unbound.kind} grant ${walletId === null ? "needs a wallet id" : "is not bound to a wallet"}`);
  checkCredential(w, credential, purposes);
  const expiresAt = secs(FIXTURE_NOW) + GRANT_TTL_SECS;
  const ownKey = keyless(w);
  return purposes.map((purpose) => {
    const grant: AuthGrant = { id: freshId(w, "9a7e"), purpose, walletId, expiresAt, singleUse: true };
    w.grants.push({ grant, used: false, ownKey });
    return grant;
  });
}

/** dw-vault's `revoke`: a lock, or an unlock that installs the data key, ends every grant. */
function revokeGrants(w: World) {
  w.grants = [];
}

/** `Vault::check_grant`: the unused grant `id` of `walletId` and of one of `kinds`, if a key
 * can use it: its own, or the vault's. */
function checkGrant(w: World, id: string, walletId: string, kinds: GrantPurposeSpec["kind"][]): FixtureGrant | null {
  const g = w.grants.find((x) => x.grant.id === id && !x.used);
  return g && g.grant.walletId === walletId && kinds.includes(g.grant.purpose.kind) && (g.ownKey || !keyless(w)) ? g : null;
}

function useGrant(w: World, id: string) {
  for (const g of w.grants) if (g.grant.id === id) g.used = true;
}

/** An unused `changeCredential` grant whose key is usable (it is bound to no wallet). */
function credentialGrant(w: World, id: string): FixtureGrant | null {
  const g = w.grants.find((x) => x.grant.id === id && !x.used);
  return g && g.grant.purpose.kind === "changeCredential" && (g.ownKey || !keyless(w)) ? g : null;
}

/** A new credential (encrypt, change, destroy): every grant goes and every live lease is revoked. */
function endVaultEpoch(ctx: Ctx, cause: "passphraseChange") {
  revokeGrants(ctx.world);
  for (const lease of ctx.world.leases.filter(live)) setLease(ctx, { ...lease, state: { kind: "revoked", cause }, keyExpiresInSecs: null });
}

function leaseChanged({ world, emit }: Ctx, lease: Lease) {
  emit({ kind: "leaseChanged", network: world.launch.network.dirName, lease });
}

function setLease(ctx: Ctx, lease: Lease): Lease {
  ctx.world.leases = ctx.world.leases.map((l) => (l.id === lease.id && l.walletId === lease.walletId ? lease : l));
  leaseChanged(ctx, lease);
  return lease;
}

const BUDGET_ORDER: Budget["purpose"][] = ["funding", "credits", "spend"];

/** Redeems `grants` into a new active lease of `flow` (lease/session.rs `issue`), listed and
 * signalled; returns its full id. Budgets: a `platformOp`'s `maxDuffs` funds, its `maxCredits`
 * credits, a `spend`'s `maxDuffs` spends; an empty one is not listed. */
function newLease(ctx: Ctx, walletId: string, flow: FlowKindName, grants: FixtureGrant[]): [string, Lease] {
  const { world } = ctx;
  const budgets: Budget[] = [];
  const budget = (purpose: Budget["purpose"], ceiling: string) => ceiling !== "0" && budgets.push({ purpose, ceiling, spent: "0" });
  for (const { grant } of grants) {
    useGrant(world, grant.id);
    if (grant.purpose.kind === "platformOp") {
      budget("funding", String(grant.purpose.maxDuffs));
      budget("credits", grant.purpose.maxCredits);
    } else if (grant.purpose.kind === "spend") budget("spend", String(grant.purpose.maxDuffs));
  }
  budgets.sort((a, b) => BUDGET_ORDER.indexOf(a.purpose) - BUDGET_ORDER.indexOf(b.purpose));
  const id = `lease-${freshId(world, "1ea5")}`;
  const ownKey = grants.some((g) => g.ownKey);
  const lease: Lease = {
    id: id.slice("lease-".length, "lease-".length + 8),
    walletId,
    flow,
    state: { kind: "active" },
    ownKey,
    keyExpiresInSecs: ownKey ? KEY_TTL_SECS : null,
    fundsCommitted: false,
    budgets,
    inFlight: 0,
    callRunning: false,
  };
  world.leases = [...world.leases, lease];
  leaseChanged(ctx, lease);
  return [id, lease];
}

/** One library call of `lease`'s flow costing `quote` credits: charged as it starts, then done. */
function leasedCall(ctx: Ctx, lease: Lease, quote: bigint): Lease {
  const budgets = lease.budgets.map((b) => (b.purpose === "credits" ? { ...b, spent: String(BigInt(b.spent) + quote) } : b));
  const running = setLease(ctx, { ...lease, budgets, callRunning: true });
  return setLease(ctx, { ...running, callRunning: false });
}

/** The engine's `write_lease` for a write of one of `flows` costing `quote` credits, in its
 * order: a lease id of the wallet's, of one of `flows`, whose Credits budget covers the quote;
 * or an unused `platformOp` vault grant of the wallet's whose `maxCredits` covers it, which
 * runs the write under a one-shot lease of `flows[0]` that ends after it. Checks only: the
 * returned commit runs the call once nothing else refuses. */
function writeLease(ctx: Ctx, walletId: string, grant: string, flows: FlowKindName[], quote: bigint): () => void {
  const { world } = ctx;
  const key = leaseKey(grant);
  if (key === null) {
    // `Vault::grant_purpose`: a grant of another kind or wallet is refused as such; a missing
    // one, or one without its own key, reports a vault without a data key first.
    const g = world.grants.find((x) => x.grant.id === grant && !x.used);
    if (g && (g.grant.walletId !== walletId || g.grant.purpose.kind !== "platformOp")) throw grantInvalid();
    if (!g?.ownKey && keyless(world)) throw fail("platform.signer_unavailable", "signer unavailable");
    if (g?.grant.purpose.kind !== "platformOp") throw grantInvalid();
    const remaining = BigInt(g.grant.purpose.maxCredits);
    if (quote > remaining) throw grantExceeded(quote, remaining);
    return () => {
      const [, lease] = newLease(ctx, walletId, flows[0], [g]);
      setLease(ctx, { ...leasedCall(ctx, lease, quote), state: { kind: "ended" }, keyExpiresInSecs: null });
    };
  }
  const lease = world.leases.find((l) => l.id === world.flowLeases[key] && l.walletId === walletId);
  if (!lease || !flows.includes(lease.flow)) throw grantInvalid();
  const credits = lease.budgets.find((b) => b.purpose === "credits" && BigInt(b.ceiling) > 0n);
  if (!credits) throw fail("platform.needs_grant", "needs a grant", { kind: "needsGrant", purpose: "credits" });
  const remaining = BigInt(credits.ceiling) - BigInt(credits.spent);
  if (quote > remaining) throw grantExceeded(quote, remaining);
  if (lease.state.kind === "ended") throw fail("platform.lease_expired", "lease expired");
  if (lease.state.kind === "revoked") throw fail("platform.lease_revoked", "lease revoked", { kind: "revoked", cause: lease.state.cause });
  if (lease.state.kind === "needsGrant") throw fail("platform.needs_grant", "needs a grant", { kind: "needsGrant", purpose: "credits" });
  return () => void leasedCall(ctx, lease, quote);
}

/** A Base58 identity id, as `Identifier::from_string` takes one: 32 bytes once decoded (each
 * leading `1` a zero byte), so 32 to 44 characters. */
function isIdentityId(s: string): boolean {
  if (!/^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(s)) return false;
  const zeros = s.length - s.replace(/^1+/, "").length;
  const n = [...s.slice(zeros)].reduce((n, c) => n * 58n + BigInt(BASE58.indexOf(c)), 0n);
  return zeros + (n === 0n ? 0 : Math.ceil(n.toString(16).length / 2)) === 32;
}

// User lookups and contact requests (DP2-02, DP2-03).

/** `resolve_user`: the label trimmed, one `.dash` dropped in any case, lower-cased; the hit with
 * its relation now. */
function lookup(dp: DashPayWorld, username: string): UserHit | null {
  if (dp.searchOffline) throw fail("platform.unavailable", "DAPI did not answer");
  const label = username.trim().replace(/\.dash$/i, "").toLowerCase();
  const hit = dp.directory.find((u) => u.username === label);
  return hit ? { ...hit, relation: relationOf(dp, hit.identity) } : null;
}

/** `verify_scanned`'s answer for the user `hit`, verified by `code`. */
const scanned = (hit: UserHit, code: ScannedContactInfo["code"]): ScannedContactInfo => ({
  identity: hit.identity,
  username: hit.username,
  displayName: hit.displayName,
  code,
  relation: hit.relation,
  unverified: hit.unverified,
});

/** DashPay::eligibility (requests.rs): the wallet's own state first, in the engine's order after
 * `isSelf` (a contact, our request out, theirs in), then the identity's own DashPay keys; only
 * then is the contact fetched: unknown is `identity.not_found`, one without DashPay keys
 * `contactCannotReceive`. */
function eligibility(dp: DashPayWorld, identity: string, id: string): EligibilityName {
  if (!isIdentityId(id)) throw fail("invalid_argument", `not an identity id: ${id}`);
  if (id === identity) return "isSelf";
  const relation = dp.contacts.find((c) => c.summary.contact === id)?.summary.relation;
  if (relation === "contact") return "alreadyContact";
  if (relation === "pendingOutgoing" || relation === "pendingIncoming") return relation;
  if (!dp.identities.find((i) => i.identity === identity)!.hasDashpayKeys) return "noDashPayKeys";
  if (dp.searchOffline) throw fail("platform.unavailable", "DAPI did not answer");
  if (!dp.directory.some((u) => u.identity === id)) throw fail("identity.not_found", `identity not found: ${id}`);
  return dp.cannotReceive.includes(id) ? "contactCannotReceive" : "ok";
}

/** `?name=value` pairs of a link's query; `null` when a name repeats (names case-insensitive). */
function queryOf(query: string): Map<string, string> | null {
  const out = new Map<string, string>();
  for (const part of query.split("&")) {
    const eq = part.indexOf("=");
    const name = (eq < 0 ? part : part.slice(0, eq)).toLowerCase();
    if (out.has(name)) return null;
    let value: string;
    try {
      value = decodeURIComponent(eq < 0 ? "" : part.slice(eq + 1));
    } catch {
      return null;
    }
    out.set(name, value);
  }
  return out;
}

/** dw-uri `DashPayUserLink::parse`: `dashpay://user?id=&username=`, scheme, host and names in any
 * case, each once and nothing else; the username trimmed, one `.dash` dropped (as iOS: this case). */
function parseUserLink(text: string): { identity: string; username: string } | null {
  const m = /^dashpay:\/\/user\?([^#]*)$/i.exec(text.trim());
  const q = m && queryOf(m[1]);
  if (!q || q.size !== 2 || !q.has("id") || !q.has("username")) return null;
  const identity = q.get("id")!;
  const username = q.get("username")!.trim().replace(/\.dash$/, "");
  return isIdentityId(identity) && username ? { identity, username } : null;
}

/** dw-uri `DashPayContactCode::parse`: `dash:?du=&dapk=`, each once and nothing else. */
function parseContactCode(text: string): { username: string; expiresAt: number } | null {
  const m = /^dash:\?([^#]*)$/i.exec(text.trim());
  const q = m && queryOf(m[1]);
  if (!q || q.size !== 2) return null;
  const du = q.get("du")?.trim().replace(/\.dash$/i, "");
  const dapk = q.get("dapk");
  return du && dapk && /^\d{1,10}$/.test(dapk) ? { username: du, expiresAt: Number(dapk) } : null;
}

// The bell (platform/notifications.rs), as vectors/notifications.json records the engine's, which
// notifications.test.ts replays: each call looks the wallet up, then refuses an id that is not
// Base58 (`invalid_argument`). Only `events` asks whether the identity is the wallet's
// (`identity.not_found`, from its contacts read); a count or a mark of another identity's rows is
// no error. Not modelled: the count is 0 until the wallet's bring-up counted it (the fixture's
// is always counted), and no write journals a row (the world's journal is the scenario's).

/** Events one page holds at most (notifications.rs `MAX_PAGE`). */
const MAX_PAGE = 200;

/** The wallet's DashPay state for `identity`, an id that must be Base58. */
function bell(w: World, walletId: string, identity: string): DashPayWorld {
  const dp = dashpay(w, walletId);
  if (!isIdentityId(identity)) throw fail("invalid_argument", `not an identity id: ${identity}`);
  return dp;
}

/** `DashPay::events`: on the first page the requests waiting, then `limit` (1–200) rows below
 * `cursor`, newest first, less an ignored sender's, split into unread and read. */
function eventPage(dp: DashPayWorld, identity: string, cursor: number | null, limit: number): DashPayEventPage {
  if (!dp.identities.some((i) => i.identity === identity)) throw fail("identity.not_found", `identity not found: ${identity}`);
  const ignored = new Set(dp.contacts.filter((c) => c.summary.relation === "ignored").map((c) => c.summary.contact));
  const below = cursor ?? Number.POSITIVE_INFINITY;
  const rows = dp.journal
    .filter((r) => r.identity === identity && r.id < below && !ignored.has(r.contact))
    .sort((a, b) => b.id - a.id);
  const page = rows.slice(0, Math.min(Math.max(limit, 1), MAX_PAGE));
  const events = page.map(eventOf);
  return {
    // As `contacts`' Requests section lists them.
    pending: cursor === null ? contactList(dp, { sections: ["requests"], sort: "dateAdded", text: null }).sections[0].contacts : [],
    new: events.filter((e) => e.readAt === null),
    earlier: events.filter((e) => e.readAt !== null),
    nextCursor: rows.length > page.length ? page[page.length - 1].id : null,
  };
}

/** Marks `identity`'s unread rows up to `upTo` read (with `contact`, only that contact's), and
 * signals its feed when one changed. */
function markRead(ctx: Ctx, walletId: string, dp: DashPayWorld, identity: string, upTo: number, contact?: string) {
  let changed = 0;
  for (const r of dp.journal)
    if (r.identity === identity && r.id <= upTo && r.readAt === null && (contact === undefined || r.contact === contact)) {
      r.readAt = secs(FIXTURE_NOW);
      changed++;
    }
  if (changed > 0)
    ctx.emit({ kind: "platform", network: ctx.world.launch.network.dirName, walletId, change: { domain: "events", identity } });
}

/** `Vault::unlock`: a wrong passphrase counts and reports dw-vault's wait (`retryAfterSecs`, also
 * in `lock_status`). The fixture's clock stands still, so the wait is reported but never
 * enforced: the screens enforce their own (U-02 D8). Installing the data key ends the epoch
 * (dw-vault `install_key`): every grant goes, and every live lease needs a grant again, its key
 * dropped (the lease table's `epoch_changed`). */
function unlock(ctx: Ctx, passphrase: string, state: "unlocked" | "unlockedMixingOnly") {
  const { world } = ctx;
  if (world.lock.state === "noVault") throw fail("vault.no_vault", "no vault");
  if (!world.lock.encrypted) throw fail("vault.not_encrypted", "vault not encrypted");
  if (passphrase !== world.passphrase) {
    const failedAttempts = world.lock.failedAttempts + 1;
    const retryAfterSecs = throttleWaitSecs(failedAttempts) || null;
    world.lock = { ...world.lock, failedAttempts, retryAfterSecs };
    throw fail("vault.wrong_passphrase", `wrong passphrase (${failedAttempts} failed)`, { kind: "wrongPassphrase", failedAttempts, retryAfterSecs });
  }
  if (world.lock.state !== "unlocked") {
    revokeGrants(world);
    for (const lease of world.leases.filter((l) => live(l) && l.state.kind !== "needsGrant"))
      setLease(ctx, { ...lease, state: { kind: "needsGrant" }, keyExpiresInSecs: null });
  }
  world.lock = { ...world.lock, failedAttempts: 0, retryAfterSecs: null };
  setLock(ctx, state);
  return world.lock;
}

/** Adds the wallet of `phrase` (`NetworkSession::import_wallet`): a phrase of a watch-only wallet
 * gives it its keys back; one of a wallet with keys is `wallet_already_exists`. */
function importWallet(ctx: Ctx, phrase: string, name: string | null): string {
  const { world } = ctx;
  if (world.lock.state === "noVault") throw fail("vault.no_vault", "no vault");
  if (world.lock.state === "locked") throw fail("vault.locked", "vault locked");
  if (world.lock.state === "unlockedMixingOnly") throw fail("vault.mixing_only", "vault unlocked for mixing only");
  if (checkPhrase(phrase).checksum === "invalid") throw fail("invalid_mnemonic", "invalid mnemonic");
  const normalized = normalizePhrase(phrase);
  const known = Object.entries(world.phrases).find(([, p]) => p === normalized)?.[0];
  const walletId = known ?? walletIdOf(normalized);
  const existing = world.wallets.find((x) => x.summary.walletId === walletId);
  if (existing && !existing.summary.watchOnly) throw fail("wallet_already_exists", "wallet already exists");
  const network = world.launch.network.dirName;
  if (existing) existing.summary = { ...existing.summary, watchOnly: false, hasMnemonic: true };
  else {
    const wallet = emptyWallet(walletId, name ?? `Wallet ${world.wallets.length + 1}`);
    world.wallets.push(wallet);
  }
  world.phrases[walletId] = normalized;
  world.lock = { ...world.lock, walletsWithSecrets: [...new Set([...world.lock.walletsWithSecrets, walletId])].sort() };
  ctx.emit(existing ? { kind: "walletLoadChanged", network, walletId, loaded: true } : { kind: "walletCreated", network, walletId });
  return walletId;
}

export const HANDLERS: Handlers = {
  // Pay and Receive (U-03): send.ts.
  ...sendHandlers({
    wallet,
    spendGrant: (w, id, walletId) => checkGrant(w, id, walletId, ["spend"]),
    useGrant,
  }),
  // Tools (U-05): tools.ts.
  ...toolsHandlers({ wallet, grant: checkGrant, useGrant }),
  // Settings (U-04): settings.ts.
  ...settingsHandlers({ wallet, grant: checkGrant, useGrant }),
  ...scanHandlers(),

  // App and network
  launch_info: (_, { world }) => world.launch,
  recent_notices: (_, { world }) => world.notices,
  open_network: (_, { world }) => ({ network: world.launch.network, lock: world.lock }),
  active_network: (_, { world }) => world.launch.network,

  // Vault, leases and the lock report (E0-04, E0-13 D3)
  // The vault's status counts its wait down (dw-vault `retry_after`) when a scenario sets one
  // (U-03); a wrong passphrase reports its own wait (U-02).
  lock_status: (_, { world }) => {
    const left = (world.throttledUntil ?? 0) - Date.now();
    return left > 0 ? { ...world.lock, retryAfterSecs: Math.ceil(left / 1000) } : world.lock;
  },
  unlock: ({ passphrase }, ctx) => unlock(ctx, passphrase, "unlocked"),
  unlock_mixing_only: ({ passphrase }, ctx) => unlock(ctx, passphrase, "unlockedMixingOnly"),
  lock: (_, ctx) => {
    const { world, emit } = ctx;
    const network = world.launch.network.dirName;
    // Revoke every live lease (the engine's lock barrier: its key goes, its `ownKey` flag
    // stays), then report what the cut did.
    for (const lease of world.leases.filter(live)) setLease(ctx, { ...lease, state: { kind: "revoked", cause: "lock" }, keyExpiresInSecs: null });
    revokeGrants(world);
    setLock(ctx, "locked");
    const inFlight = world.leases.reduce((n, l) => n + l.inFlight, 0);
    if (inFlight > 0) emit({ kind: "lockProgress", network, progress: { kind: "draining", inFlight, deadlineInMs: 30_000 } });
    // As dw-app's bridge: the first 16 wallets with secrets, and how many more.
    const all = world.lock.walletsWithSecrets;
    const report: LockOutcome = {
      status: { ...world.lock, walletsWithSecrets: all.slice(0, MAX_REPORT_WALLETS) },
      ...world.cut,
      walletsWithSecretsOmitted: Math.max(0, all.length - MAX_REPORT_WALLETS),
    };
    world.lastLockReport = report;
    emit({ kind: "lockProgress", network, progress: { kind: "done", report } });
    return report;
  },
  last_lock_report: (_, { world }) => world.lastLockReport,
  create_vault: ({ passphrase }, ctx) => {
    const { world } = ctx;
    if (world.lock.state !== "noVault") throw fail("vault.already_exists", "vault already exists");
    if (passphrase !== null) world.passphrase = passphrase;
    world.lock = { ...world.lock, encrypted: passphrase !== null, failedAttempts: 0, retryAfterSecs: null, walletsWithSecrets: [] };
    setLock(ctx, passphrase === null ? "noKeys" : "unlocked");
    return world.lock;
  },
  generate_mnemonic: ({ wordCount, language }) => {
    if (language !== "english" || (wordCount !== 12 && wordCount !== 24)) throw fail("invalid_argument", "the fixture makes English phrases of 12 or 24 words");
    return wordCount === 12 ? CREATED_12 : CREATED_24;
  },
  check_mnemonic: ({ phrase }) => checkPhrase(phrase),
  reveal_mnemonic: ({ walletId, grantId }, { world }) => {
    if (!checkGrant(world, grantId, walletId, ["revealSecret"])) throw fail("vault.grant_invalid", "grant invalid");
    useGrant(world, grantId);
    const phrase = world.phrases[walletId];
    if (!phrase) throw fail("vault.no_secret", "no secret for this wallet");
    return { phrase, bip39Passphrase: "" };
  },
  recover_with_mnemonic: ({ walletId, mnemonic, newPassphrase }, ctx) => {
    const { world } = ctx;
    if (world.lock.state === "noVault") throw fail("vault.no_vault", "no vault");
    if (world.phrases[walletId] !== normalizePhrase(mnemonic)) throw fail("vault.recovery_mismatch", "the phrase does not derive this wallet");
    const walletsWithoutSecrets = world.lock.walletsWithSecrets.filter((w) => w !== walletId);
    for (const w of world.wallets)
      if (walletsWithoutSecrets.includes(w.summary.walletId)) w.summary = { ...w.summary, watchOnly: true, hasMnemonic: false };
    world.phrases = { [walletId]: world.phrases[walletId] };
    world.passphrase = newPassphrase;
    // A new vault: no grant or lease of the old one survives (E0-13 D3: `recoverWithMnemonic` revokes every lease).
    revokeGrants(world);
    for (const lease of world.leases.filter(live)) setLease(ctx, { ...lease, state: { kind: "revoked", cause: "passphraseChange" }, keyExpiresInSecs: null });
    world.lock = { ...world.lock, encrypted: true, failedAttempts: 0, retryAfterSecs: null, walletsWithSecrets: [walletId] };
    setLock(ctx, "unlocked");
    for (const id of walletsWithoutSecrets) ctx.emit({ kind: "walletLoadChanged", network: world.launch.network.dirName, walletId: id, loaded: true });
    return { status: world.lock, walletsWithoutSecrets };
  },
  // Settings ▸ Security (U-04): the vault's credential.
  encrypt_vault: ({ newPassphrase, grantId }, ctx) => {
    const { world } = ctx;
    if (world.lock.state === "noVault") throw fail("vault.no_vault", "no vault");
    if (world.lock.encrypted) throw fail("vault.already_encrypted", "vault already encrypted");
    if (!credentialGrant(world, grantId)) throw fail("vault.grant_invalid", "grant invalid");
    if (newPassphrase === "") throw fail("vault.passphrase_rejected", "the passphrase is empty");
    useGrant(world, grantId);
    world.passphrase = newPassphrase;
    endVaultEpoch(ctx, "passphraseChange");
    world.lock = { ...world.lock, encrypted: true, failedAttempts: 0, retryAfterSecs: null };
    setLock(ctx, "locked");
    return world.lock;
  },
  change_passphrase: ({ oldPassphrase, newPassphrase }, ctx) => {
    const { world } = ctx;
    if (world.lock.state === "noVault") throw fail("vault.no_vault", "no vault");
    if (!world.lock.encrypted) throw fail("vault.not_encrypted", "vault not encrypted");
    // DEC-134: a wrong old passphrase counts like a wrong unlock and revokes nothing.
    checkCredential(world, { kind: "passphrase", passphrase: oldPassphrase }, []);
    if (newPassphrase === "") throw fail("vault.passphrase_rejected", "the passphrase is empty");
    world.passphrase = newPassphrase;
    endVaultEpoch(ctx, "passphraseChange");
    return world.lock;
  },
  set_payment_authentication: ({ on, grantId }, { world }) => {
    if (world.lock.state === "noVault") throw fail("vault.no_vault", "no vault");
    if (!world.lock.encrypted) throw fail("vault.not_encrypted", "vault not encrypted");
    if (!credentialGrant(world, grantId)) throw fail("vault.grant_invalid", "grant invalid");
    useGrant(world, grantId);
    world.paymentAuth = on;
    return null;
  },
  destroy_vault: ({ credential }, ctx) => {
    const { world } = ctx;
    if (world.lock.state === "noVault") return world.lock;
    if (world.lock.walletsWithSecrets.length > 0) throw fail("vault.not_empty", "the vault still holds wallet secrets");
    if (world.lock.encrypted && credential.kind !== "passphrase") throw fail("vault.credential_required", "the passphrase is required");
    checkCredential(world, credential, []);
    endVaultEpoch(ctx, "passphraseChange");
    world.passphrase = "";
    world.paymentAuth = true;
    world.lock = { ...world.lock, encrypted: false, quickUnlockEnrolled: false, failedAttempts: 0, retryAfterSecs: null, walletsWithSecrets: [] };
    setLock(ctx, "noVault");
    return world.lock;
  },
  leases: (_, { world }) => world.leases,
  authorize: ({ purpose, walletId, credential }, { world }) => authorizeSet(world, [purpose], walletId, credential)[0],
  authorize_set: ({ purposes, walletId, credential }, { world }) => authorizeSet(world, purposes, walletId, credential),
  revoke_grant: ({ grantId }, { world }) => {
    useGrant(world, grantId);
    return null;
  },
  // NetworkSession::begin_flow: every grant checked before any is redeemed (lease/session.rs
  // `issue`): unused, the wallet's, usable by a key, one per kind, of a kind the flow allows (a
  // `platformOp`, and for "Accept and pay" a `spend` too; a contact payment's only a `spend`).
  begin_flow: ({ walletId, flow, grants }, ctx) => {
    const { world } = ctx;
    wallet(world, walletId);
    const kinds: GrantPurposeSpec["kind"][] =
      flow === "acceptAndPay" ? ["platformOp", "spend"] : flow === "contactPayment" ? ["spend"] : ["platformOp"];
    const found = grants.map((id) => checkGrant(world, id, walletId, kinds));
    const purposes = new Set(found.map((g) => g?.grant.purpose.kind));
    if (!found.length || found.some((g) => !g) || purposes.size !== found.length) throw grantInvalid();
    const [id, lease] = newLease(ctx, walletId, flow, found as FixtureGrant[]);
    world.flowLeases[id] = lease.id;
    return id;
  },
  // Idempotent; a lease id that does not parse is `platform.grant_invalid`. Ending a live lease
  // signals it twice, as the engine does (the owner's drop, then the table's `end`).
  end_flow: ({ lease: id }, ctx) => {
    const key = leaseKey(id);
    if (key === null) throw grantInvalid();
    const lease = ctx.world.leases.find((l) => l.id === ctx.world.flowLeases[key]);
    if (lease && lease.state.kind !== "ended") {
      const ended = setLease(ctx, { ...lease, state: { kind: "ended" }, keyExpiresInSecs: null });
      leaseChanged(ctx, ended);
    }
    return null;
  },
  dispatch_status: ({ walletId, artifact }, { world }) => {
    wallet(world, walletId);
    // The engine's DashPay::dispatch_status: a txid, a transition hash or a step id.
    if (!isCheckable(artifact)) throw fail("invalid_argument", "not a txid, transition hash or step id");
    return world.dispatch[artifact] ?? null;
  },

  // The shell (U-01): links are queued already routed (FixtureHandle.deliverLinks).
  take_links: ({ network }, { world }) => {
    if (network !== world.launch.network.dirName) throw fail("invalid_argument", `not the window's network: ${network}`);
    return world.links.splice(0);
  },
  route_link: ({ text }) => routeLink(text),
  shell_sync: ({ sync }, { world }) => {
    world.shellSync = sync;
    return null;
  },

  // Wallets and chain
  list_wallets: (_, { world }) => world.wallets.filter((x) => !isClosed(world, x.summary.walletId)).map((x) => x.summary),
  wallet_info: ({ walletId }, { world }) => wallet(world, walletId).summary,
  balance: ({ walletId }, { world }) => wallet(world, walletId).summary.balances,
  import_wallet: ({ mnemonic, options }, ctx) => importWallet(ctx, mnemonic, options.name),
  sync_status: (_, { world }) => world.sync,
  history_page: ({ walletId, request }, { world }) => historyPage(wallet(world, walletId).rows, request),
  tx_detail: ({ walletId, txid }, { world }) => {
    const row = wallet(world, walletId).rows.find((r) => r.txid === txid);
    if (!row) throw fail("tx_not_found", `transaction not found: ${txid}`);
    return txDetails(row);
  },

  // DashPay: identities and registration
  dashpay_status: ({ walletId }, { world }) => dashpay(world, walletId).status,
  // A pass is asked for; the fixture's world is always past it.
  dashpay_sync_soon: () => null,
  dashpay_startup: ({ walletId }, { world }) => dashpay(world, walletId).startup,
  identities: ({ walletId }, { world }) => dashpay(world, walletId).identities,
  identity_detail: ({ walletId, identity }, { world }) => ({
    summary: dashpay(world, walletId, identity).identities.find((i) => i.identity === identity)!,
    revision: 3,
    publicKeys: [],
  }),
  main_name: ({ walletId, identity }, { world }) =>
    dashpay(world, walletId, identity).identities.find((i) => i.identity === identity)!.mainName,
  registrations: ({ walletId }, { world }) => dashpay(world, walletId).registrations,
  search_users: ({ walletId, prefix, limit }, { world }) => {
    const dp = dashpay(world, walletId);
    if (dp.searchOffline) throw fail("platform.unavailable", "DAPI did not answer");
    return dp.directory
      .filter((u) => u.username.startsWith(prefix.trim().toLowerCase()))
      .slice(0, limit)
      .map((u) => ({ ...u, relation: relationOf(dp, u.identity) }));
  },
  resolve_user: ({ walletId, username }, { world }) => lookup(dashpay(world, walletId), username),

  // DashPay: contacts
  dashpay_contacts: ({ walletId, identity, query }, { world }) => contactList(dashpay(world, walletId, identity), query),
  dashpay_contact: ({ walletId, identity, contact: id }, { world }) =>
    dashpay(world, walletId, identity).contacts.find((c) => c.summary.contact === id) ?? null,
  dashpay_pending_setup_count: ({ walletId }, { world }) =>
    dashpay(world, walletId).contacts.filter((c) => c.summary.channel === "setupPending").length,
  dashpay_eligibility: ({ walletId, identity, contact: id }, { world }) => eligibility(dashpay(world, walletId, identity), identity, id),
  // DashPay::send_request (requests.rs `send`), in the engine's order: the ids, the scan's claim,
  // the identity, eligibility, its credits, then the grant (`writeLease`), consumed only once the
  // write goes out; from there the scan is spent. The request is a new outgoing one, `pending`.
  dashpay_send_request: ({ walletId, identity, to, scan, grant }, ctx) => {
    const { world } = ctx;
    const dp0 = dashpay(world, walletId);
    if (!isIdentityId(identity) || !isIdentityId(to)) throw fail("invalid_argument", "not an identity id");
    let claim: FixtureScan | undefined;
    if (scan !== null) {
      claim = dp0.scans.find((x) => x.id === scan && !x.spent);
      if (!claim) throw fail("contact.scan_expired", "scan expired");
      if (claim.recipient !== to) throw fail("invalid_argument", "the scan is for another identity");
    }
    const dp = dashpay(world, walletId, identity);
    const reason = eligibility(dp, identity, to);
    if (reason !== "ok") throw fail("contact.ineligible", `contact ineligible: ${reason}`, { kind: "ineligible", reason });
    const me = dp.identities.find((i) => i.identity === identity)!;
    const available = me.balance ?? "0";
    if (BigInt(available) < CONTACT_REQUEST_CREDITS)
      throw fail("platform.insufficient_credits", "insufficient credits", { kind: "credits", needed: String(CONTACT_REQUEST_CREDITS), available });
    // The claim needs a minute left once the grant is in (`SCAN_MARGIN_SECS`).
    const consume = writeLease(ctx, walletId, grant, ["contactRequest"], CONTACT_REQUEST_CREDITS);
    if (claim && claim.expiresAt <= secs(FIXTURE_NOW) + SCAN_MARGIN_SECS) throw fail("contact.scan_expired", "scan expired");
    consume();
    if (claim) claim.spent = true;
    const hit = dp.directory.find((u) => u.identity === to)!;
    const createdAt = FIXTURE_NOW.getTime();
    const since = Math.floor(createdAt / 1000);
    // An ignored sender's request is in: the library joins the two (`established`).
    const ignored = dp.contacts.find((c) => c.summary.contact === to);
    if (ignored) {
      dp.requestTimes[to] = { incoming: dp.requestTimes[to]?.incoming ?? null, outgoing: createdAt };
      Object.assign(ignored.summary, { relation: "contact", hidden: false, channel: "ready", lastActivityAt: since });
      ignored.requestSentAt = since;
      contactsChanged(ctx, walletId, identity);
      return { kind: "established", request: String(createdAt) };
    }
    dp.requestTimes[to] = { incoming: null, outgoing: createdAt };
    dp.contacts.push({
      summary: {
        contact: to,
        username: hit.username,
        displayName: hit.displayName,
        alias: null,
        relation: "pendingOutgoing",
        hidden: false,
        channel: "notEstablished",
        since,
        lastActivityAt: since,
        unverified: hit.unverified,
      },
      profile: null,
      privateDetails: { alias: null, note: null, hidden: false },
      publishState: "local",
      requestSentAt: since,
      requestReceivedAt: null,
      paymentLock: null,
    });
    contactsChanged(ctx, walletId, identity);
    return { kind: "pending", request: String(createdAt) };
  },
  // DashPay::accept_request (requests.rs `accept`), in the engine's order: the ids, itself, the
  // identity, a contact already, their request, its DashPay keys, its credits, then the grant
  // (`writeLease`), consumed only once the write goes out. The request it returns is the
  // `$createdAt` (decimal ms) of the reverse request it writes (journal.rs `request_ref`).
  dashpay_accept_request: ({ walletId, identity, from, grant }, ctx) => {
    const { world } = ctx;
    wallet(world, walletId);
    if (!isIdentityId(identity) || !isIdentityId(from)) throw fail("invalid_argument", "not an identity id");
    if (from === identity) throw fail("contact.self", "contact is the identity itself");
    const dp = dashpay(world, walletId, identity);
    const me = dp.identities.find((i) => i.identity === identity)!;
    const c = dp.contacts.find((x) => x.summary.contact === from);
    if (c?.summary.relation === "contact") throw fail("contact.already_contact", "already a contact");
    if (c?.summary.relation !== "pendingIncoming") throw fail("invalid_argument", "no contact request from this identity");
    if (!me.hasDashpayKeys) throw fail("contact.ineligible", "contact ineligible: NoDashPayKeys", { kind: "ineligible", reason: "noDashPayKeys" });
    // The engine reads the balance from Platform; the fixture's summary is its copy.
    const quote = CONTACT_REQUEST_CREDITS;
    const available = me.balance ?? "0";
    if (BigInt(available) < quote) throw fail("platform.insufficient_credits", "insufficient credits", { kind: "credits", needed: String(quote), available });
    writeLease(ctx, walletId, grant, ["accept", "acceptAndPay"], quote)();
    const createdAt = FIXTURE_NOW.getTime();
    dp.requestTimes[from] = { incoming: dp.requestTimes[from]?.incoming ?? null, outgoing: createdAt };
    const since = Math.floor(createdAt / 1000);
    c.summary = { ...c.summary, relation: "contact", channel: "setupPending", since };
    c.requestSentAt = since;
    contactsChanged(ctx, walletId, identity);
    return { kind: "established", request: String(createdAt) };
  },
  // An ignore drops the request and mutes the sender: their unread rows are marked read.
  dashpay_ignore_contact: ({ walletId, identity, contact: id }, ctx) => {
    const c = contact(ctx.world, walletId, identity, id);
    c.summary = { ...c.summary, relation: "ignored" };
    contactsChanged(ctx, walletId, identity);
    markRead(ctx, walletId, dashpay(ctx.world, walletId), identity, Number.MAX_SAFE_INTEGER, id);
    return null;
  },
  // The request is back at once: platform-wallet refetches it on its next sweep, which the
  // fixture's world is always past. The sender's rows stay read.
  dashpay_unignore_contact: ({ walletId, identity, contact: id }, ctx) => {
    const c = contact(ctx.world, walletId, identity, id);
    c.summary = { ...c.summary, relation: "pendingIncoming" };
    contactsChanged(ctx, walletId, identity);
    return null;
  },
  // DashPay::set_private_details (DP2-04, m4 §2.4): established contacts only; trimmed, blank is
  // none; fewer than two contacts defer the publish; without a grant an edit of published details
  // needs one and is not saved; with one, unchanged published details send nothing.
  dashpay_set_private_details: ({ walletId, identity, contact: id, details, grant }, ctx) => {
    if (id === identity) throw fail("contact.self", "contact is the identity itself");
    // The size first, then the relation, as the engine checks them.
    const clean = (v: string | null) => v?.trim() || null;
    const d: PrivateContactDetails = { alias: clean(details.alias), note: clean(details.note), hidden: details.hidden };
    const bytes = new TextEncoder().encode(JSON.stringify({ alias: d.alias, note: d.note })).length;
    if (bytes > DETAILS_MAX_BYTES)
      throw fail("contact.details_too_long", "contact details too long", { kind: "size", bytes, max: DETAILS_MAX_BYTES });
    const c = contact(ctx.world, walletId, identity, id);
    if (c.summary.relation !== "contact") throw fail("invalid_argument", "not a contact of this identity");
    const dp = dashpay(ctx.world, walletId, identity);
    const same = JSON.stringify(d) === JSON.stringify(c.privateDetails);
    const save = (state: ContactDetailInfo["publishState"]) => {
      c.privateDetails = d;
      c.publishState = state;
      c.summary = { ...c.summary, alias: d.alias, hidden: d.hidden };
      contactsChanged(ctx, walletId, identity);
      return state;
    };
    if (dp.contacts.filter((x) => x.summary.relation === "contact").length < 2) return save("deferredUntilTwoContacts");
    if (grant === null) {
      if (c.publishState === "published" && !same) throw fail("platform.needs_grant", "needs a grant", { kind: "needsGrant", purpose: "credits" });
      return save(c.publishState === "published" ? "published" : "local");
    }
    if (same && c.publishState === "published") return "published";
    const me = dp.identities.find((i) => i.identity === identity)!;
    if (!me.hasDashpayKeys) throw fail("contact.ineligible", "contact ineligible: NoDashPayKeys", { kind: "ineligible", reason: "noDashPayKeys" });
    if (BigInt(me.balance ?? "0") < DOCUMENT_10K_CREDITS)
      throw fail("platform.insufficient_credits", "insufficient credits", { kind: "credits", needed: String(DOCUMENT_10K_CREDITS), available: me.balance ?? "0" });
    writeLease(ctx, walletId, grant, ["privateDetails"], DOCUMENT_10K_CREDITS)();
    return save("published");
  },
  contact_payment_lock: ({ walletId, identity, contact: id }, { world }) => contact(world, walletId, identity, id).paymentLock,
  frequent_contacts: ({ walletId, identity, limit }, { world }) =>
    dashpay(world, walletId, identity)
      .contacts.filter((c) => c.summary.relation === "contact" && c.summary.lastActivityAt !== null)
      .sort((a, b) => b.summary.lastActivityAt! - a.summary.lastActivityAt!)
      .slice(0, limit)
      .map((c) => c.summary),
  // DashPay::contact_activity (DP3-02): the ids, itself, the identity's wallet, then the page; a
  // contact never paid is an empty page.
  contact_activity: ({ walletId, identity, contact: id, cursor, filter }, { world }) => {
    wallet(world, walletId);
    if (!isIdentityId(identity) || !isIdentityId(id)) throw fail("invalid_argument", "not an identity id");
    if (id === identity) throw fail("contact.self", "contact is the identity itself");
    const dp = dashpay(world, walletId, identity);
    try {
      return activityPage(dp.activity[id] ?? [], cursor, filter, id);
    } catch {
      throw fail("invalid_argument", "not a cursor of this query");
    }
  },
  // DashPay::enable_dashpay_keys (DP2-02): nothing missing sends nothing and leaves the grant;
  // else the balance against the quote, the grant, then the identity has its keys.
  dashpay_enable_keys: ({ walletId, identity, grant }, ctx) => {
    const dp = dashpay(ctx.world, walletId, identity);
    const me = dp.identities.find((i) => i.identity === identity)!;
    if (me.hasDashpayKeys) return null;
    if (BigInt(me.balance ?? "0") < KEY_UPDATE_CREDITS)
      throw fail("platform.insufficient_credits", "insufficient credits", { kind: "credits", needed: String(KEY_UPDATE_CREDITS), available: me.balance ?? "0" });
    writeLease(ctx, walletId, grant, ["enableDashPayKeys"], KEY_UPDATE_CREDITS)();
    dp.identities = dp.identities.map((i) => (i.identity === identity ? { ...i, hasDashpayKeys: true } : i));
    ctx.emit({ kind: "platform", network: ctx.world.launch.network.dirName, walletId, change: { domain: "identities" } });
    return null;
  },
  // DashPay::grant_request (flows.rs): only an empty identity is refused; `registerName` is
  // DP1-03's quote, which the fixture does not model.
  grant_request: ({ identity, action }) => {
    if (!identity.trim()) throw fail("invalid_argument", "identity is empty");
    if (action.kind === "registerName") throw fail("platform.not_implemented", "the fixture does not quote names", { kind: "call", call: "grant_request(registerName)" });
    return { maxDuffs: 0, maxCredits: String(GRANT_CREDITS[action.kind]) };
  },
  credits_cost_table: ({ walletId }, { world }) => {
    wallet(world, walletId);
    return COST_TABLE;
  },
  // DashPay::my_user_link (users.rs): the identity's main name as iOS's link; none is
  // `contact.no_username`, names being updated `platform.unavailable`.
  dashpay_my_user_link: ({ walletId, identity }, { world }) => {
    const me = dashpay(world, walletId, identity).identities.find((i) => i.identity === identity)!;
    if (me.namesUpdating) throw fail("platform.unavailable", "names are being updated");
    if (!me.mainName) throw fail("contact.no_username", "the identity has no username");
    return `dashpay://user?id=${identity}&username=${encodeURIComponent(me.mainName)}`;
  },
  // DashPay::verify_scanned (users.rs): a user link must name the identity that owns its username
  // now; a DIP-15 code must not have expired (checked before any query), and its username must
  // exist. Anything else is `invalid_argument`, which never quotes the text. The fixture's `dapk`
  // is its expiry in UNIX seconds, not the 38-byte key blob.
  dashpay_verify_scanned: ({ walletId, text }, { world }) => {
    const dp = dashpay(world, walletId);
    const link = parseUserLink(text);
    if (link) {
      const hit = lookup(dp, link.username);
      if (hit?.identity !== link.identity) throw fail("contact.scan_unverified", "the scanned code could not be verified");
      return scanned(hit, { kind: "userLink" });
    }
    const code = parseContactCode(text);
    if (!code) throw fail("invalid_argument", "not a DashPay contact code");
    if (code.expiresAt <= secs(FIXTURE_NOW)) throw fail("contact.scan_expired", "scan expired");
    const hit = lookup(dp, code.username);
    if (!hit) throw fail("contact.scan_unverified", "the scanned code could not be verified");
    const id = `scan-${dp.scans.length + 1}`;
    dp.scans.push({ id, recipient: hit.identity, expiresAt: code.expiresAt, spent: false });
    return scanned(hit, { kind: "autoAcceptClaim", scan: id, expiresAt: code.expiresAt });
  },
  dashpay_profile: ({ walletId, identity }, { world }) =>
    dashpay(world, walletId, identity).identities.find((i) => i.identity === identity)!.profile,

  // DashPay: notifications (the bell, above).
  dashpay_events: ({ walletId, identity, cursor, limit }, { world }) => eventPage(bell(world, walletId, identity), identity, cursor, limit),
  dashpay_unread_count: ({ walletId, identity }, { world }) =>
    bell(world, walletId, identity).journal.filter((r) => r.identity === identity && r.readAt === null).length,
  dashpay_mark_read: ({ walletId, identity, upTo }, ctx) => {
    markRead(ctx, walletId, bell(ctx.world, walletId, identity), identity, upTo);
    return null;
  },
};
