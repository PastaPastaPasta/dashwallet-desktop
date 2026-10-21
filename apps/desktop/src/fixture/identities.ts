// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// Settings ▸ Advanced ▸ Identities (DP6-01): each identity's keys and revision, Refresh and
// Refresh all, the look-up, Set as main, Set as main name and Find my identities, as the engine's
// `identity_reads.rs` and `identity.rs` answer them; scenario `dashpay-identities`.

import type { AppError } from "../api/generated/commands";
import type { FoundIdentity } from "../api/generated/FoundIdentity";
import type { GrantPurposeSpec } from "../api/generated/GrantPurposeSpec";
import type { IdentityDetail } from "../api/generated/IdentityDetail";
import type { IdentityKeyInfo } from "../api/generated/IdentityKeyInfo";
import type { IdentitySummary } from "../api/generated/IdentitySummary";
import { IDENTITY, registered, type DashPayWorld } from "./dashpay";
import type { Ctx, Handlers } from "./handlers";
import type { FixtureGrant, World } from "./scenarios";

const fail = (code: AppError["code"], message: string, data: AppError["data"] = null): AppError => ({ code, message, data });

/** Platform's side of the wallet's identities and of any identity the look-up finds: keys,
 * revision, and the refreshes that fail (by identity, with their code). */
export interface IdentitiesWorld {
  keys: Record<string, IdentityKeyInfo[]>;
  revisions: Record<string, number>;
  /** What Find my identities finds next: identities of the seed past the highest on file. */
  discoverable: IdentitySummary[];
  refreshFailures: Record<string, AppError["code"]>;
}

export const identitiesOf = (dp: DashPayWorld): IdentitiesWorld => (dp.identityState ??= { keys: {}, revisions: {}, discoverable: [], refreshFailures: {} });

/** The second identity of `dashpay-identities` (index 1, the main one by choice). */
export const WORK_IDENTITY = "AWrk7m2Lq9XzDhcP4fT6bN8sGyJ3uVeR5kHnC1aQwMpE";
/** The seed's third identity, which Find my identities finds. */
export const SPARE_IDENTITY = "SpRe3vK8tQ1mZxWb6nH9cF4jL7dGyP2aUeT5sNrB3kXw";
const DASHPAY_CONTRACT = "Bwr4WHCPz5rFVAD87RqTs3izo4zpzwsEdKPWUT1NS1C7";

function key(id: number, purpose: IdentityKeyInfo["purpose"], securityLevel: IdentityKeyInfo["securityLevel"], patch: Partial<IdentityKeyInfo> = {}): IdentityKeyInfo {
  return {
    id,
    purpose,
    securityLevel,
    keyType: "ecdsaSecp256k1",
    publicKey: `0${2 + (id % 2)}${(id + 10).toString(16).repeat(32)}`,
    readOnly: false,
    disabledAt: null,
    contractBound: null,
    contractBoundDocumentType: null,
    ...patch,
  };
}

/** A registration's keys (MASTER, HIGH, CRITICAL auth, a transfer key), plus DashPay's
 * encryption and decryption keys bound to its contact requests when `dashpay`. */
export function identityKeys(dashpay: boolean): IdentityKeyInfo[] {
  const keys = [key(0, "authentication", "master"), key(1, "authentication", "high"), key(2, "authentication", "critical"), key(3, "transfer", "critical")];
  if (!dashpay) return keys;
  const bound = { contractBound: DASHPAY_CONTRACT, contractBoundDocumentType: "contactRequest" };
  return [...keys, key(4, "encryption", "medium", bound), key(5, "decryption", "medium", bound)];
}

/** `dashpay-identities`: Alice (index 0) and her work identity (index 1), the main one by
 * choice; one of Alice's keys disabled; a third identity of the seed for Find my identities. */
export function identitiesWorld(): DashPayWorld {
  const world = registered();
  const alice = world.identities[0];
  const work: IdentitySummary = {
    ...alice,
    identity: WORK_IDENTITY,
    index: 1,
    names: ["alicework", "aliceatwork"],
    mainName: "alicework",
    isMain: true,
    balance: "52000000000",
    profile: null,
    hasDashpayKeys: false,
  };
  const aliceKeys = identityKeys(true);
  aliceKeys.push(key(6, "authentication", "high", { disabledAt: 1_760_000_000_000 }));
  world.status = { kind: "ready", main: WORK_IDENTITY };
  world.startup = { ...world.startup, identity: WORK_IDENTITY };
  world.identities = [{ ...alice, isMain: false }, work];
  world.directory = [...world.directory, { identity: WORK_IDENTITY, username: "alicework", displayName: null, avatarUrl: null, relation: "isSelf", unverified: false }];
  world.identityState = {
    keys: { [IDENTITY.alice]: aliceKeys, [WORK_IDENTITY]: identityKeys(false), [IDENTITY.bob]: identityKeys(true) },
    revisions: { [IDENTITY.alice]: 3, [WORK_IDENTITY]: 1, [IDENTITY.bob]: 2 },
    discoverable: [
      { ...work, identity: SPARE_IDENTITY, index: 2, names: [], mainName: null, isMain: false, balance: "1000000", creditLevel: "low" },
    ],
    refreshFailures: {},
  };
  return world;
}

export interface IdentitiesDeps {
  dashpay(w: World, walletId: string, identity?: string): DashPayWorld;
  grant(w: World, id: string, walletId: string, kinds: GrantPurposeSpec["kind"][]): FixtureGrant | null;
  useGrant(w: World, id: string): void;
}

const BASE58_ID = /^[1-9A-HJ-NP-Za-km-z]{42,44}$/;

export function identitiesHandlers(d: IdentitiesDeps): Handlers {
  const changed = ({ world, emit }: Ctx, walletId: string) =>
    emit({ kind: "platform", network: world.launch.network.dirName, walletId, change: { domain: "identities" } });

  function detail(dp: DashPayWorld, identity: string): IdentityDetail {
    const s = identitiesOf(dp);
    return { summary: dp.identities.find((i) => i.identity === identity)!, revision: s.revisions[identity] ?? null, publicKeys: s.keys[identity] ?? [] };
  }

  /** Refresh of one identity: its failure, else its balance current again. */
  function refresh(dp: DashPayWorld, identity: string) {
    const code = identitiesOf(dp).refreshFailures[identity];
    if (code) throw fail(code, `refresh of ${identity} failed`);
    dp.identities = dp.identities.map((i) => (i.identity === identity ? { ...i, balanceStale: false } : i));
  }

  return {
    identity_detail: ({ walletId, identity }, { world }) => detail(d.dashpay(world, walletId, identity), identity),
    identity_refresh: ({ walletId, identity }, ctx) => {
      const dp = d.dashpay(ctx.world, walletId, identity);
      refresh(dp, identity);
      changed(ctx, walletId);
      return detail(dp, identity);
    },
    identity_refresh_all: ({ walletId }, ctx) => {
      const dp = d.dashpay(ctx.world, walletId);
      const out = [...dp.identities]
        .sort((a, b) => a.index - b.index)
        .map(({ identity }) => {
          try {
            refresh(dp, identity);
            return { identity, failureCode: null };
          } catch (e) {
            return { identity, failureCode: (e as AppError).code as string };
          }
        });
      changed(ctx, walletId);
      return out;
    },
    identity_find: ({ walletId, query }, { world }): FoundIdentity | null => {
      const dp = d.dashpay(world, walletId);
      if (dp.searchOffline) throw fail("platform.unavailable", "DAPI did not answer");
      const byId = BASE58_ID.test(query);
      const label = query.replace(/\.dash$/i, "").toLowerCase();
      if (!byId && !/^[a-z0-9][a-z0-9-]{1,61}[a-z0-9]$/.test(label)) throw fail("invalid_argument", "not an identity id or a username");
      const own = dp.identities.find((i) => (byId ? i.identity === query : i.names.includes(label)));
      const hit = dp.directory.find((u) => (byId ? u.identity === query : u.username === label));
      const identity = own?.identity ?? hit?.identity;
      if (!identity) return null;
      const s = identitiesOf(dp);
      return {
        identity,
        foundBy: byId ? { kind: "id" } : { kind: "name", label },
        names: own ? own.names : [hit!.username],
        balance: own?.balance ?? "250000000",
        revision: s.revisions[identity] ?? 1,
        publicKeys: s.keys[identity] ?? identityKeys(true),
        own: own !== undefined,
        unverified: own?.unverified ?? hit!.unverified,
      };
    },
    set_main_identity: ({ walletId, identity }, ctx) => {
      const dp = d.dashpay(ctx.world, walletId, identity);
      dp.identities = dp.identities.map((i) => ({ ...i, isMain: i.identity === identity }));
      if (dp.status.kind === "ready") dp.status = { kind: "ready", main: identity };
      changed(ctx, walletId);
      return null;
    },
    set_main_name: ({ walletId, identity, label }, ctx) => {
      const dp = d.dashpay(ctx.world, walletId, identity);
      const me = dp.identities.find((i) => i.identity === identity)!;
      if (label !== null && !me.names.includes(label)) throw fail("invalid_argument", "not a name this identity owns");
      dp.identities = dp.identities.map((i) => (i.identity === identity ? { ...i, mainName: label ?? i.names[0] ?? null } : i));
      changed(ctx, walletId);
      return null;
    },
    discover_identities: ({ walletId, grant }, ctx) => {
      const dp = d.dashpay(ctx.world, walletId);
      if (!d.grant(ctx.world, grant, walletId, ["identityScan"])) throw fail("platform.grant_invalid", "grant invalid");
      d.useGrant(ctx.world, grant);
      const s = identitiesOf(dp);
      const found = s.discoverable;
      s.discoverable = [];
      dp.identities = [...dp.identities, ...found];
      if (found.length > 0) changed(ctx, walletId);
      return found.length;
    },
  };
}
