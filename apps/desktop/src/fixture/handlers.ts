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
import { eventOf, sectionOf, type DashPayWorld } from "./dashpay";
import { routeLink } from "./links";
import { sendHandlers } from "./send";
import { MAX_REPORT_WALLETS, type FixtureGrant, type World } from "./scenarios";
import { BASE58, FIXTURE_NOW, historyPage, secs, txDetails } from "./wallet";

export interface Ctx {
  world: World;
  emit(event: AppEvent): void;
}

export type Handlers = { [K in CommandName]?: (args: CommandArgs[K], ctx: Ctx) => CommandResults[K] };

const fail = (code: AppError["code"], message: string, data: AppError["data"] = null): AppError => ({ code, message, data });

function wallet(w: World, walletId: string) {
  const found = w.wallets.find((x) => x.summary.walletId === walletId);
  if (!found) throw fail("wallet_not_found", `wallet not found: ${walletId}`);
  return found;
}

/** The wallet's DashPay state, for one of its identities. */
function dashpay(w: World, walletId: string, identity?: string) {
  wallet(w, walletId);
  if (identity !== undefined && !w.dashpay.identities.some((i) => i.identity === identity))
    throw fail("identity.not_found", `identity not found: ${identity}`);
  return w.dashpay;
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
  if (credential.kind === "unencrypted" && w.lock.encrypted && w.paymentAuth && purposes.some((p) => p.kind === "spend"))
    throw fail("vault.credential_required", "every payment needs authentication");
  switch (credential.kind) {
    case "passphrase": {
      if (credential.passphrase !== w.passphrase) {
        // As `unlock` counts a wrong passphrase.
        const failedAttempts = w.lock.failedAttempts + 1;
        w.lock = { ...w.lock, failedAttempts };
        throw fail("vault.wrong_passphrase", `wrong passphrase (${failedAttempts} failed)`, {
          kind: "wrongPassphrase",
          failedAttempts,
          retryAfterSecs: null,
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

/** The engine's `DateAdded` order of requests: the newest first, unknown times last, then by id. */
const byDateAdded = (a: { since: number | null; contact: string }, b: { since: number | null; contact: string }) =>
  (a.since === null ? 1 : 0) - (b.since === null ? 1 : 0) || (b.since ?? 0) - (a.since ?? 0) || (a.contact < b.contact ? -1 : a.contact > b.contact ? 1 : 0);

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
    pending: cursor === null ? dp.contacts.filter((c) => sectionOf(c) === "requests").map((c) => c.summary).sort(byDateAdded) : [],
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

export const HANDLERS: Handlers = {
  // Pay and Receive (U-03): send.ts.
  ...sendHandlers({
    wallet,
    spendGrant: (w, id, walletId) => checkGrant(w, id, walletId, ["spend"]),
    useGrant,
  }),

  // App and network
  launch_info: (_, { world }) => world.launch,
  recent_notices: (_, { world }) => world.notices,
  open_network: (_, { world }) => ({ network: world.launch.network, lock: world.lock }),
  active_network: (_, { world }) => world.launch.network,

  // Vault, leases and the lock report (E0-04, E0-13 D3)
  lock_status: (_, { world }) => world.lock,
  unlock: ({ passphrase }, ctx) => {
    const { world } = ctx;
    if (passphrase !== world.passphrase) {
      const failedAttempts = world.lock.failedAttempts + 1;
      world.lock = { ...world.lock, failedAttempts };
      throw fail("vault.wrong_passphrase", `wrong passphrase (${failedAttempts} failed)`, {
        kind: "wrongPassphrase",
        failedAttempts,
        retryAfterSecs: null,
      });
    }
    // Installing the data key ends the epoch (dw-vault `install_key`): every grant goes, and
    // every live lease needs a grant again, its key dropped (the lease table's `epoch_changed`).
    if (world.lock.state !== "unlocked") {
      revokeGrants(world);
      for (const lease of world.leases.filter((l) => live(l) && l.state.kind !== "needsGrant"))
        setLease(ctx, { ...lease, state: { kind: "needsGrant" }, keyExpiresInSecs: null });
    }
    world.lock = { ...world.lock, failedAttempts: 0 };
    setLock(ctx, "unlocked");
    return world.lock;
  },
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
  leases: (_, { world }) => world.leases,
  authorize: ({ purpose, walletId, credential }, { world }) => authorizeSet(world, [purpose], walletId, credential)[0],
  authorize_set: ({ purposes, walletId, credential }, { world }) => authorizeSet(world, purposes, walletId, credential),
  revoke_grant: ({ grantId }, { world }) => {
    useGrant(world, grantId);
    return null;
  },
  // NetworkSession::begin_flow: every grant checked before any is redeemed (lease/session.rs
  // `issue`): unused, the wallet's, usable by a key, one per kind, of a kind the flow allows (a
  // `platformOp`, and for "Accept and pay" a `spend` too).
  begin_flow: ({ walletId, flow, grants }, ctx) => {
    const { world } = ctx;
    wallet(world, walletId);
    const kinds: GrantPurposeSpec["kind"][] = flow === "acceptAndPay" ? ["platformOp", "spend"] : ["platformOp"];
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
  list_wallets: (_, { world }) => world.wallets.map((x) => x.summary),
  wallet_info: ({ walletId }, { world }) => wallet(world, walletId).summary,
  balance: ({ walletId }, { world }) => wallet(world, walletId).summary.balances,
  sync_status: (_, { world }) => world.sync,
  history_page: ({ walletId, request }, { world }) => historyPage(wallet(world, walletId).rows, request),
  tx_detail: ({ walletId, txid }, { world }) => {
    const row = wallet(world, walletId).rows.find((r) => r.txid === txid);
    if (!row) throw fail("tx_not_found", `transaction not found: ${txid}`);
    return txDetails(row);
  },

  // DashPay: identities and registration
  dashpay_status: ({ walletId }, { world }) => dashpay(world, walletId).status,
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
  search_users: ({ walletId, prefix, limit }, { world }) =>
    dashpay(world, walletId)
      .directory.filter((u) => u.username.startsWith(prefix.toLowerCase()))
      .slice(0, limit),
  resolve_user: ({ walletId, username }, { world }) =>
    dashpay(world, walletId).directory.find((u) => u.username === username.toLowerCase()) ?? null,

  // DashPay: contacts
  dashpay_contacts: ({ walletId, identity, query }, { world }) => {
    const text = query.text?.trim().toLowerCase();
    const contacts = dashpay(world, walletId, identity).contacts.filter(
      (c) => !text || [c.summary.username, c.summary.displayName, c.summary.alias].some((v) => v?.toLowerCase().includes(text)),
    );
    const sections = query.sections.length ? query.sections : (["requests", "contacts", "pending", "hidden"] as const);
    return {
      sections: sections.map((section) => ({ section, contacts: contacts.filter((c) => sectionOf(c) === section).map((c) => c.summary) })),
      updating: false,
    };
  },
  dashpay_contact: ({ walletId, identity, contact: id }, { world }) =>
    dashpay(world, walletId, identity).contacts.find((c) => c.summary.contact === id) ?? null,
  dashpay_pending_setup_count: ({ walletId }, { world }) =>
    dashpay(world, walletId).contacts.filter((c) => c.summary.channel === "setupPending").length,
  dashpay_eligibility: ({ walletId, identity, contact: id }, { world }) => {
    if (id === identity) return "isSelf";
    const relation = dashpay(world, walletId, identity).contacts.find((c) => c.summary.contact === id)?.summary.relation;
    if (relation === "contact") return "alreadyContact";
    if (relation === "pendingOutgoing" || relation === "pendingIncoming") return relation;
    return "ok";
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
    markRead(ctx, walletId, ctx.world.dashpay, identity, Number.MAX_SAFE_INTEGER, id);
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
  dashpay_set_private_details: ({ walletId, identity, contact: id, details }, ctx) => {
    const c = contact(ctx.world, walletId, identity, id);
    c.privateDetails = details;
    c.summary = { ...c.summary, alias: details.alias, hidden: details.hidden };
    contactsChanged(ctx, walletId, identity);
    return c.publishState;
  },
  contact_payment_lock: ({ walletId, identity, contact: id }, { world }) => contact(world, walletId, identity, id).paymentLock,
  frequent_contacts: ({ walletId, identity, limit }, { world }) =>
    dashpay(world, walletId, identity)
      .contacts.filter((c) => c.summary.relation === "contact" && c.summary.lastActivityAt !== null)
      .sort((a, b) => b.summary.lastActivityAt! - a.summary.lastActivityAt!)
      .slice(0, limit)
      .map((c) => c.summary),
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
