// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// App Connect ("Sign in to an app") for the fixture: dw-app's inbox of requests by id
// (shell/links.rs `ConnectLinks`) and the `app_connect_*` commands, for a made-up app, "Yappr".
// Each fixture link id stands for one way a request goes (CONNECT_LINKS); the real requests,
// their checks against Platform and the grant binding are dw-engine's and tested there.

import type { AppError } from "../api/generated/commands";
import type { ConnectBinding } from "../api/generated/ConnectBinding";
import type { ConnectChoice } from "../api/generated/ConnectChoice";
import type { ConnectSheet } from "../api/generated/ConnectSheet";
import type { GrantPurposeSpec } from "../api/generated/GrantPurposeSpec";
import type { LinkRoute } from "../api/generated/LinkRoute";
import type { DashPayWorld } from "./dashpay";
import type { Handlers } from "./handlers";
import type { FixtureGrant, World } from "./scenarios";
import { FIXTURE_NOW } from "./wallet";

const fail = (code: AppError["code"], message: string, data: AppError["data"] = null): AppError => ({ code, message, data });

/** One fixture request per outcome, by its inbox id (32 hex digits, as dw-app's). */
export const CONNECT_LINKS = {
  ok: "a99c0000000000000000000000000001",
  mainnetOff: "a99c0000000000000000000000000002",
  sendFailed: "a99c0000000000000000000000000003",
  planChanged: "a99c0000000000000000000000000004",
  stale: "a99c0000000000000000000000000005",
  failed: "a99c0000000000000000000000000006",
  credits: "a99c0000000000000000000000000007",
  notConfirmed: "a99c0000000000000000000000000008",
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
   * went unconfirmed (`notConfirmed`). */
  reread: boolean;
  /** The choice of an approval that went unconfirmed, which a sheet read again offers. */
  earlier: ConnectChoice | null;
  /** `app_connect_return` opened its return link. */
  returned: boolean;
  /** `app_connect_dismiss` forgot it (kept here for the tests to read). */
  dismissed: boolean;
}

/** The host a fixture request delivered by the OS returns to. */
export const CONNECT_HOST = "yappr.example";

const base58 = (seed: string) => (seed + "1111111111111111111111111111111111111111111").slice(0, 44);
const POSTS = base58("7YpprPostsCntrctAq3vK8tQ");
const LIKES = base58("8YpprLikesCntrctBm9xWb6n");
const OWNER = base58("5YpprwnerMk2Lq9XzDhcP4fT");
const ADMIN = base58("6YpprAdmnRt3vZxWb6nH9cF4");
const GROUP = base58("9YpprGrpEq1mZxWb6nH9cF4j");
const APP_ID = "a7c3e5f1".repeat(8);
const REQUEST = "5e2d".repeat(10);
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

const connectOf = (w: World) => (w.connect ??= {});

/** Notes an App Connect route in the inbox, under its id; other routes pass as they are. */
export function takeConnect(w: World, route: LinkRoute): LinkRoute {
  if (route.kind !== "appConnect") return route;
  const outcome = (Object.keys(CONNECT_LINKS) as ConnectOutcome[]).find((k) => CONNECT_LINKS[k] === route.linkId) ?? "ok";
  connectOf(w)[route.linkId] = { outcome, returnsTo: route.returnsTo, answer: null, started: false, keysAdded: null, reread: false, earlier: null, returned: false, dismissed: false };
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

function bindingsOf(l: FixtureConnect): ConnectBinding[] {
  return l.outcome === "planChanged" && l.reread ? BINDINGS.slice(0, 2) : BINDINGS;
}

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
    if (l.outcome === "mainnetOff") throw refuse(l, fail("connect.mainnet_off", "connect.mainnet_off"));
    if (l.outcome === "stale") throw refuse(l, fail("connect.stale", "connect.stale"));
    if (l.answer !== null) throw fail("connect.answered", "connect.answered");
    const identities = dashpay(w, walletId)
      .identities.filter((i) => i.mainName !== null)
      .map((i) => ({ identity: i.identity, username: i.mainName!, balance: i.balance }));
    if (identities.length === 0) throw refuse(l, fail("connect.no_identity", "connect.no_identity"));
    const now = FIXTURE_NOW.getTime();
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
    app_connect_sheet: ({ walletId, linkId }, { world }) => sheetOf(world, walletId, linkId),
    app_connect_quote: ({ walletId, linkId, choice }, { world }) => {
      const { who } = checkChoice(world, walletId, linkId, choice);
      const fee = feeOf(choice);
      const balance = link(world, linkId).outcome === "credits" ? fee / 2n : BigInt(who.balance ?? "0");
      if (balance < fee) throw fail("connect.credits", "connect.credits", { kind: "credits", needed: String(fee), available: String(balance) });
      return {
        grant: { kind: "appConnect", maxCredits: String(fee), request: REQUEST, plan: planOf(choice) },
        fee: String(fee),
        budgets: [choice.terms.fundingLimit],
        balance: String(balance),
      };
    },
    app_connect_approve: ({ walletId, linkId, choice, grant: id }, { world }) => {
      const l = link(world, linkId);
      if (l.started || l.answer !== null) throw fail("connect.answered", "connect.answered");
      const { who } = checkChoice(world, walletId, linkId, choice);
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
  };
}
