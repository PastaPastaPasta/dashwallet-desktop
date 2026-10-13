// The fixture engine: dw-app's commands over a `World`. Each handler takes the command's
// generated `CommandArgs` and returns its `CommandResults`, so a binding change the fixture
// does not follow fails `tsc`. Mutating commands emit the `AppEvent`s the engine would.
// A command without a handler rejects with `platform.not_implemented` (installFixture).

import type { AppError, AppEvent, CommandArgs, CommandName, CommandResults } from "../api/generated/commands";
import type { ContactDetailInfo } from "../api/generated/ContactDetailInfo";
import type { LockOutcome } from "../api/generated/LockOutcome";
import { IDENTITY, sectionOf } from "./dashpay";
import type { World } from "./scenarios";
import { historyPage, txDetails } from "./wallet";

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

export const HANDLERS: Handlers = {
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
    world.lock = { ...world.lock, failedAttempts: 0 };
    setLock(ctx, "unlocked");
    return world.lock;
  },
  lock: (_, ctx) => {
    const { world, emit } = ctx;
    const network = world.launch.network.dirName;
    // Revoke every live lease (the engine's lock barrier), then report what the cut did.
    world.leases = world.leases.map((l) => (l.state.kind === "ended" ? l : { ...l, state: { kind: "revoked", cause: "lock" }, ownKey: false, keyExpiresInSecs: null }));
    for (const lease of world.leases) emit({ kind: "leaseChanged", network, lease });
    setLock(ctx, "locked");
    const report: LockOutcome = { status: world.lock, ...world.cut };
    world.lastLockReport = report;
    emit({ kind: "lockProgress", network, progress: { kind: "done", report } });
    return report;
  },
  last_lock_report: (_, { world }) => world.lastLockReport,
  leases: (_, { world }) => world.leases,
  dispatch_status: ({ walletId, artifact }, { world }) => {
    wallet(world, walletId);
    return world.dispatch[artifact] ?? null;
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
  dashpay_accept_request: ({ walletId, identity, from }, ctx) => {
    const c = contact(ctx.world, walletId, identity, from);
    if (c.summary.relation !== "pendingIncoming") throw fail("invalid_argument", `no request from ${from}`);
    c.summary = { ...c.summary, relation: "contact", channel: "setupPending", since: Math.floor(ctx.world.sync.tipTime ?? 0) };
    contactsChanged(ctx, walletId, identity);
    return { kind: "established", request: `req-${c.summary.username}` };
  },
  dashpay_ignore_contact: ({ walletId, identity, contact: id }, ctx) => {
    const c = contact(ctx.world, walletId, identity, id);
    c.summary = { ...c.summary, relation: "ignored" };
    contactsChanged(ctx, walletId, identity);
    return null;
  },
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

  // DashPay: notifications
  dashpay_events: ({ walletId, identity, cursor, limit }, { world }) => {
    const dp = dashpay(world, walletId, identity);
    const older = dp.events.filter((e) => cursor === null || e.id < cursor);
    const page = older.slice(0, limit);
    return {
      pending: dp.contacts.filter((c) => c.summary.relation === "pendingIncoming").map((c) => c.summary),
      new: page.filter((e) => e.readAt === null),
      earlier: page.filter((e) => e.readAt !== null),
      nextCursor: older.length > limit ? page[page.length - 1].id : null,
    };
  },
  dashpay_unread_count: ({ walletId, identity }, { world }) => dashpay(world, walletId, identity).events.filter((e) => e.readAt === null).length,
  dashpay_mark_read: ({ walletId, identity, upTo }, ctx) => {
    const dp = dashpay(ctx.world, walletId, identity);
    const now = ctx.world.sync.tipTime ?? 0;
    dp.events = dp.events.map((e) => (e.id <= upTo && e.readAt === null ? { ...e, readAt: now } : e));
    contactsChanged(ctx, walletId, identity);
    return null;
  },
};

/** The identity every DashPay scenario's wallet uses as its main one. */
export const MAIN_IDENTITY = IDENTITY.alice;
