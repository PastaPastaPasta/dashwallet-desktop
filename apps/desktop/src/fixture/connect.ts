// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// App Connect ("Sign in to an app") for the fixture: dw-app's inbox of requests by id
// (shell/links.rs `ConnectLinks`) and the `app_connect_*` commands, for a made-up app, "Yappr".
// Each fixture link id stands for one way a request goes (CONNECT_LINKS); the real requests,
// their checks against Platform and the grant binding are dw-engine's and tested there.

import type { AppError } from "../api/generated/commands";
import type { ConnectBinding } from "../api/generated/ConnectBinding";
import type { ConnectChoice } from "../api/generated/ConnectChoice";
import type { ConnectSheet } from "../api/generated/ConnectSheet";
import type { ConnectedApp } from "../api/generated/ConnectedApp";
import type { GrantPurposeSpec } from "../api/generated/GrantPurposeSpec";
import type { LinkRoute } from "../api/generated/LinkRoute";
import type { DashPayWorld } from "./dashpay";
import type { Handlers } from "./handlers";
import type { FixtureGrant, World } from "./scenarios";
import { FIXTURE_NOW } from "./wallet";

/** Scripted outcome for `app_connect_disconnect_quote` / `app_connect_disconnect` in the fixture. */
export type DisconnectFixtureOutcome =
  | "ok"
  | "responseInFlight"
  | "notConfirmed"
  | "unverifiedKey"
  | "feeTooHigh"
  | "pendingFeeTooHigh"
  | "pendingWrite"
  | "grantExceeded"
  | "disconnectConflict"
  | "priorChargeKnown"
  | "priorChargePossible"
  | "credits"
  | "error";

const DISCONNECT_FEE = "22000000";

const fail = (code: AppError["code"], message: string, data: AppError["data"] = null): AppError => ({ code, message, data });

/** One fixture request per outcome, by its inbox id (32 hex digits, as dw-app's). */
export const CONNECT_LINKS = {
  ok: "a99c0000000000000000000000000001",
  sendFailed: "a99c0000000000000000000000000003",
  planChanged: "a99c0000000000000000000000000004",
  stale: "a99c0000000000000000000000000005",
  failed: "a99c0000000000000000000000000006",
  credits: "a99c0000000000000000000000000007",
  notConfirmed: "a99c0000000000000000000000000008",
  /** Reopened after an approval that went unconfirmed, while Platform can't be reached: its first
   * quote fails, the next one succeeds. */
  quoteOffline: "a99c0000000000000000000000000009",
  /** Its group gains an admin between the sheet's read and the first quote's. */
  groupChanged: "a99c000000000000000000000000000a",
  /** Its group facts change before each of the first two quotes, and the read of the request
   * after the second fails once with `platform.unavailable`. */
  groupRefreshOffline: "a99c000000000000000000000000000b",
} as const;

export type ConnectOutcome = keyof typeof CONNECT_LINKS;

/** A request in the inbox and its answer so far, as `ConnectLink`. */
export interface FixtureConnect {
  outcome: ConnectOutcome;
  returnsTo: string | null;
  answer: "ok" | "declined" | "refused" | null;
  started: boolean;
  /** The keys an approval added, once it did. */
  keysAdded: number[] | null;
  /** The request was read again since Platform changed it (`planChanged`), or its first approval
   * went unconfirmed (`notConfirmed`); its group gained an admin (`groupChanged`); its one failed
   * read happened (`groupRefreshOffline`). */
  reread: boolean;
  /** The choice of an approval that went unconfirmed, which a sheet read again offers. */
  earlier: ConnectChoice | null;
  /** `app_connect_return` opened its return link. */
  returned: boolean;
  /** `app_connect_dismiss` forgot it (kept here for the tests to read). */
  dismissed: boolean;
  /** Quotes refused so far (`quoteOffline`). */
  quotesFailed: number;
  /** Times the group facts changed so far (`groupRefreshOffline`). */
  groupChanges: number;
}

/** The host a fixture request delivered by the OS returns to. */
export const CONNECT_HOST = "yappr.example";

const base58 = (seed: string) => (seed + "1111111111111111111111111111111111111111111").slice(0, 44);
const POSTS = base58("7YpprPostsCntrctAq3vK8tQ");
const LIKES = base58("8YpprLikesCntrctBm9xWb6n");
const OWNER = base58("5YpprwnerMk2Lq9XzDhcP4fT");
const ADMIN = base58("6YpprAdmnRt3vZxWb6nH9cF4");
const NEW_ADMIN = base58("4YpprNewAdmnQ2wXcT7kL8pR");
const GROUP = base58("9YpprGrpEq1mZxWb6nH9cF4j");
const APP_ID = "a7c3e5f1".repeat(8);
const FORUM_APP_ID = "b8d4f6e2".repeat(8);
const REQUEST = "5e2d".repeat(10);
const REQUEST_YAPPR = "1a2b".repeat(10);
const REQUEST_FORUM = "3c4d".repeat(10);
const DAY_MS = 86_400_000;
const FUNDING_LIMIT = "5000000000";
const MAX_FUNDING_LIMIT = "500000000000";
const KEY_CREDITS = 6_500_000n;
const BASE_CREDITS = 20_000_000n;

const BINDINGS: ConnectBinding[] = [
  { index: 0, primary: true, level: "high", scope: { kind: "documentType", contract: POSTS, documentType: "post" } },
  { index: 1, primary: false, level: "critical", scope: { kind: "profile" } },
  { index: 2, primary: false, level: "critical", scope: { kind: "group", group: { group: GROUP, owner: OWNER, admins: [ADMIN], contracts: [POSTS, LIKES], documentTypes: [], tokens: [] } } },
];

/** Sample connected apps for fixture scenarios (fixture data only). */
export function sampleConnectedApps(identity: string, now = FIXTURE_NOW.getTime()): ConnectedApp[] {
  return [
    {
      request: REQUEST_YAPPR,
      appId: APP_ID,
      label: "Yappr",
      returnsTo: CONNECT_HOST,
      identity,
      keys: [
        {
          keyId: 6,
          level: "critical",
          scope: BINDINGS[2].scope,
          budget: "4800000000",
          expiresAtMs: now + 30 * DAY_MS,
        },
        {
          keyId: 7,
          level: "high",
          scope: { kind: "profile" },
          budget: "200000000",
          expiresAtMs: now + 30 * DAY_MS,
        },
      ],
      fundingLimit: FUNDING_LIMIT,
      expiresAtMs: now + 30 * DAY_MS,
      approvedAtMs: now - 2 * DAY_MS,
    },
    {
      request: REQUEST_FORUM,
      appId: FORUM_APP_ID,
      label: "Dash Forum",
      returnsTo: null,
      identity,
      keys: [
        {
          keyId: 8,
          level: "high",
          scope: { kind: "documentType", contract: POSTS, documentType: "post" },
          budget: "1000000000",
          expiresAtMs: now + 14 * DAY_MS,
        },
      ],
      fundingLimit: "1000000000",
      expiresAtMs: now + 14 * DAY_MS,
      approvedAtMs: now - 5 * DAY_MS,
    },
  ];
}

const connectOf = (w: World) => (w.connect ??= {});

/** Notes an App Connect route in the inbox, under its id; other routes pass as they are. */
export function takeConnect(w: World, route: LinkRoute): LinkRoute {
  if (route.kind !== "appConnect") return route;
  const outcome = (Object.keys(CONNECT_LINKS) as ConnectOutcome[]).find((k) => CONNECT_LINKS[k] === route.linkId) ?? "ok";
  connectOf(w)[route.linkId] = { outcome, returnsTo: route.returnsTo, answer: null, started: false, keysAdded: null, reread: false, earlier: null, returned: false, dismissed: false, quotesFailed: 0, groupChanges: 0 };
  return route;
}

/** A `dashpay://connect` request typed into Open URI: the `ok` request, which returns nowhere. */
export function connectRoute(text: string): LinkRoute | null {
  return /^dashpay:\/\/connect\?/i.test(text.trim()) ? { kind: "appConnect", linkId: CONNECT_LINKS.ok, returnsTo: null } : null;
}

function link(w: World, linkId: string): FixtureConnect {
  const found = connectOf(w)[linkId];
  if (!found || found.dismissed) throw fail("invalid_argument", "invalid argument: no such sign-in request");
  return found;
}

/** A refusal that answers the request, as `answered` records its status. */
function refuse(l: FixtureConnect, error: AppError): AppError {
  l.answer ??= "refused";
  return error;
}

/** The group facts as Platform has them now: `groupChanged`'s and `groupRefreshOffline`'s group gained an admin,
 * and `groupRefreshOffline`'s changed once more (its digest only). */
const groupGrew = (l: FixtureConnect) => (l.outcome === "groupChanged" && l.reread) || l.groupChanges > 0;

function bindingsOf(l: FixtureConnect): ConnectBinding[] {
  if (l.outcome === "planChanged" && l.reread) return BINDINGS.slice(0, 2);
  if (!groupGrew(l)) return BINDINGS;
  return BINDINGS.map((b): ConnectBinding => (b.scope.kind === "group" ? { ...b, scope: { kind: "group", group: { ...b.scope.group, admins: [ADMIN, NEW_ADMIN] } } } : b));
}

/** The digest of the group facts a read found, as 32 bytes of hex. */
const groupsOf = (l: FixtureConnect) => (l.groupChanges === 2 ? "7b" : groupGrew(l) ? "6a" : "5f").repeat(32);

/** The plan a quote binds its grant to: what the choice adds, as 32 bytes of hex. */
function planOf(choice: ConnectChoice): string {
  return (choice.selection.join("") + "f" + choice.terms.fundingLimit + "e" + choice.terms.expiresAtMs.toString(16)).padEnd(64, "0").slice(0, 64);
}

function feeOf(choice: ConnectChoice): bigint {
  return BASE_CREDITS + KEY_CREDITS * BigInt(choice.selection.length + 1);
}

export interface ConnectDeps {
  wallet: (w: World, walletId: string) => unknown;
  dashpay: (w: World, walletId: string) => DashPayWorld;
  grant: (w: World, id: string, walletId: string, kinds: GrantPurposeSpec["kind"][]) => FixtureGrant | null;
  useGrant: (w: World, id: string) => void;
}

export function connectHandlers({ wallet, dashpay, grant, useGrant }: ConnectDeps): Handlers {
  const sheetOf = (w: World, walletId: string, linkId: string): ConnectSheet => {
    const l = link(w, linkId);
    wallet(w, walletId);
    if (l.outcome === "stale") throw refuse(l, fail("connect.stale", "connect.stale"));
    if (l.answer !== null) throw fail("connect.answered", "connect.answered");
    const identities = dashpay(w, walletId)
      .identities.filter((i) => i.mainName !== null)
      .map((i) => ({ identity: i.identity, username: i.mainName!, balance: i.balance }));
    if (identities.length === 0) throw refuse(l, fail("connect.no_identity", "connect.no_identity"));
    const now = FIXTURE_NOW.getTime();
    if (l.outcome === "quoteOffline")
      l.earlier ??= { identity: identities[0]!.identity, selection: [0, 1, 2], terms: { fundingLimit: FUNDING_LIMIT, expiresAtMs: now + 30 * DAY_MS } };
    return {
      request: REQUEST,
      appId: APP_ID,
      label: "Yappr",
      appOwner: OWNER,
      returnsTo: l.returnsTo,
      identities,
      bindings: bindingsOf(l),
      encryption: true,
      terms: { fundingLimit: FUNDING_LIMIT, expiresAtMs: now + 30 * DAY_MS },
      maxFundingLimit: MAX_FUNDING_LIMIT,
      earlierApproval: l.earlier,
      blockTimeMs: now,
      groups: groupsOf(l),
    };
  };

  const checkChoice = (w: World, walletId: string, linkId: string, choice: ConnectChoice) => {
    const sheet = sheetOf(w, walletId, linkId);
    const who = sheet.identities.find((i) => i.identity === choice.identity);
    if (!who) throw fail("connect.hint", "connect.hint");
    const primary = sheet.bindings.find((b) => b.primary)!;
    if (!choice.selection.includes(primary.index) || choice.selection.some((i) => !sheet.bindings.some((b) => b.index === i)))
      throw fail("invalid_argument", "invalid argument: selection");
    if (!/^\d+$/.test(choice.terms.fundingLimit) || BigInt(choice.terms.fundingLimit) === 0n) throw fail("invalid_argument", "invalid argument: funding limit");
    return { sheet, who };
  };

  return {
    app_connect_sheet: ({ walletId, linkId }, { world }) => {
      const l = link(world, linkId);
      if (l.outcome === "groupRefreshOffline" && l.groupChanges === 2 && !l.reread) {
        l.reread = true;
        throw fail("platform.unavailable", "platform unavailable");
      }
      return sheetOf(world, walletId, linkId);
    },
    app_connect_quote: ({ walletId, linkId, choice }, { world }) => {
      const { who } = checkChoice(world, walletId, linkId, choice);
      const l = link(world, linkId);
      if (l.outcome === "quoteOffline" && l.quotesFailed++ === 0) throw fail("platform.unavailable", "platform unavailable");
      if (l.outcome === "groupChanged") l.reread = true;
      if (l.outcome === "groupRefreshOffline" && l.groupChanges < 2) l.groupChanges++;
      const fee = feeOf(choice);
      const balance = l.outcome === "credits" ? fee / 2n : BigInt(who.balance ?? "0");
      if (balance < fee) throw fail("connect.credits", "connect.credits", { kind: "credits", needed: String(fee), available: String(balance) });
      return {
        grant: { kind: "appConnect", maxCredits: String(fee), request: REQUEST, plan: planOf(choice) },
        fee: String(fee),
        budgets: [choice.terms.fundingLimit],
        balance: String(balance),
        groups: groupsOf(l),
      };
    },
    app_connect_approve: ({ walletId, linkId, choice, grant: id }, { world }) => {
      const l = link(world, linkId);
      if (l.started || l.answer !== null) throw fail("connect.answered", "connect.answered");
      const { sheet, who } = checkChoice(world, walletId, linkId, choice);
      const g = grant(world, id, walletId, ["appConnect"]);
      if (!g || g.grant.purpose.kind !== "appConnect" || g.grant.purpose.plan !== planOf(choice)) throw fail("platform.grant_invalid", "grant invalid");
      if (l.outcome === "planChanged" && !l.reread) {
        l.reread = true;
        throw fail("connect.plan_changed", "connect.plan_changed");
      }
      useGrant(world, id);
      l.started = true;
      if (l.outcome === "failed") throw refuse(l, fail("connect.failed", "connect.failed", { kind: "connectFailure", failure: "keyConflict" }));
      if (l.outcome === "notConfirmed" && !l.reread) {
        l.reread = true;
        l.earlier = choice;
        l.started = false;
        throw fail("connect.failed", "connect.failed", { kind: "connectFailure", failure: "notConfirmed" });
      }
      l.keysAdded = choice.selection.map((_, i) => 5 + i);
      const addedKeys = choice.selection.map((idx, i) => {
        const b = sheet.bindings.find((x) => x.index === idx) ?? sheet.bindings[0];
        return {
          keyId: 5 + i,
          level: b.level,
          scope: b.scope,
          budget: choice.terms.fundingLimit,
          expiresAtMs: choice.terms.expiresAtMs,
        };
      });
      const existing = (world.connectedApps ??= []);
      existing.unshift({
        request: sheet.request,
        appId: sheet.appId,
        label: sheet.label,
        returnsTo: l.returnsTo,
        identity: who.identity,
        keys: addedKeys,
        fundingLimit: choice.terms.fundingLimit,
        expiresAtMs: choice.terms.expiresAtMs,
        approvedAtMs: FIXTURE_NOW.getTime(),
      });
      if (l.outcome === "sendFailed") throw fail("connect.send_failed", "connect.send_failed");
      l.answer = "ok";
      return { identity: who.identity, keysAdded: l.keysAdded };
    },
    app_connect_retry_send: ({ walletId, linkId }, { world }) => {
      wallet(world, walletId);
      const l = link(world, linkId);
      if (l.keysAdded === null) throw fail("connect.failed", "connect.failed", { kind: "connectFailure", failure: "nothingToSend" });
      l.answer = "ok";
      const identity = dashpay(world, walletId).identities.find((i) => i.mainName !== null)?.identity ?? "";
      return { identity, keysAdded: l.keysAdded };
    },
    app_connect_deny: ({ linkId }, { world }) => {
      const l = link(world, linkId);
      if (l.started || l.answer !== null) throw fail("connect.answered", "connect.answered");
      l.answer = "declined";
      return null;
    },
    app_connect_return: ({ linkId }, { world }) => {
      const l = link(world, linkId);
      if (l.returnsTo === null || l.answer === null) throw fail("invalid_argument", "invalid argument: no return link to open");
      l.returned = true;
      return null;
    },
    app_connect_dismiss: ({ linkId }, { world }) => {
      const l = connectOf(world)[linkId];
      if (l) l.dismissed = true;
      return null;
    },
    app_connect_list: ({ walletId, identity }, { world }) => {
      wallet(world, walletId);
      if (world.connectedAppsListError) throw fail("platform.unavailable", "platform.unavailable");
      const exists = dashpay(world, walletId).identities.some((i) => i.identity === identity);
      if (!exists) throw fail("identity.not_found", "identity.not_found");
      const apps = (world.connectedApps ?? []).filter((a) => a.identity === identity && a.keys.length > 0);
      return { identity, apps, blockTimeMs: FIXTURE_NOW.getTime() };
    },
    app_connect_disconnect_quote: ({ walletId, identity, request }, { world }) => {
      wallet(world, walletId);
      const who = dashpay(world, walletId).identities.find((i) => i.identity === identity);
      if (!who) throw fail("identity.not_found", "identity.not_found");
      const app = (world.connectedApps ?? []).find((a) => a.identity === identity && a.request === request);
      if (!app) throw fail("invalid_argument", "invalid argument: unknown app connection");
      const outcome = world.disconnectOutcome ?? "ok";
      if (outcome === "unverifiedKey") throw fail("connect.failed", "connect.failed", { kind: "connectFailure", failure: "unverifiedKey" });
      if (outcome === "feeTooHigh") throw fail("connect.failed", "connect.failed", { kind: "connectFailure", failure: "feeTooHigh" });
      const fee = BigInt(DISCONNECT_FEE);
      const balance = outcome === "credits" ? fee / 2n : BigInt(who.balance ?? "0");
      if (balance < fee) throw fail("connect.credits", "connect.credits", { kind: "credits", needed: String(fee), available: String(balance) });
      return {
        grant: { kind: "appDisconnect", maxCredits: String(fee), identity, request },
        identity,
        keys: app.keys.map((k) => k.keyId),
        fee: String(fee),
        balance: String(balance),
        priorAttemptConflict: outcome === "priorChargeKnown" || outcome === "priorChargePossible",
        priorAttemptDebit: outcome === "priorChargeKnown" ? "500000" : null,
      };
    },
    app_connect_disconnect: ({ walletId, identity, request, grant: id }, { world }) => {
      wallet(world, walletId);
      const who = dashpay(world, walletId).identities.find((i) => i.identity === identity);
      if (!who) throw fail("identity.not_found", "identity.not_found");
      const apps = world.connectedApps ?? [];
      const idx = apps.findIndex((a) => a.identity === identity && a.request === request);
      if (idx < 0) throw fail("invalid_argument", "invalid argument: unknown app connection");
      const g = grant(world, id, walletId, ["appDisconnect"]);
      if (!g || g.grant.purpose.kind !== "appDisconnect" || g.grant.purpose.identity !== identity || g.grant.purpose.request !== request)
        throw fail("platform.grant_invalid", "grant invalid");
      const outcome = world.disconnectOutcome ?? "ok";
      if (outcome === "unverifiedKey") throw fail("connect.failed", "connect.failed", { kind: "connectFailure", failure: "unverifiedKey" });
      if (outcome === "feeTooHigh") throw fail("connect.failed", "connect.failed", { kind: "connectFailure", failure: "feeTooHigh" });
      if (outcome === "pendingFeeTooHigh") throw fail("connect.failed", "connect.failed", { kind: "connectFailure", failure: "pendingFeeTooHigh" });
      if (outcome === "pendingWrite") throw fail("connect.failed", "connect.failed", { kind: "connectFailure", failure: "pendingWrite" });
      if (outcome === "grantExceeded")
        throw fail("platform.grant_exceeded", "platform.grant_exceeded", { kind: "budget", purpose: "credits", needed: "25000000", remaining: DISCONNECT_FEE });
      if (outcome === "disconnectConflict") throw fail("connect.failed", "connect.failed", { kind: "connectFailure", failure: "disconnectConflict" });
      if (outcome === "credits") throw fail("connect.credits", "connect.credits", { kind: "credits", needed: DISCONNECT_FEE, available: "10000000" });
      useGrant(world, id);
      if (outcome === "notConfirmed") throw fail("connect.failed", "connect.failed", { kind: "connectFailure", failure: "notConfirmed" });
      if (outcome === "error") throw fail("platform.unavailable", "platform.unavailable");
      const [removed] = apps.splice(idx, 1);
      return { identity, keysDisabled: removed.keys.map((k) => k.keyId), responseMayHaveLanded: outcome === "responseInFlight" };
    },
  };
}
