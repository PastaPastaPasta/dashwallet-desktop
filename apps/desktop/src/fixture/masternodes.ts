// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The fixture's masternodes (MG-10 §5.1 over mn-engine §2.1–§2.2 and the keychain, m3-engine §2.5):
// the SPV list, the main wallet's masternodes and the tracked ones, payouts, attached keys and the
// keychain. Testnet-shaped (port 19999, `y…` addresses, documentation IPs). The commands refuse in
// the contract's order; a change `emit`s `masternodes` as the engine's notifier would. Keys are
// fixed fake test vectors: `KEY_*` (repeated hex bytes) and Bitcoin Core's test WIF.

import type { AppError } from "../api/generated/commands";
import type { AttachedKey } from "../api/generated/AttachedKey";
import type { AttachedKeyRoleName } from "../api/generated/AttachedKeyRoleName";
import type { CollateralCoin } from "../api/generated/CollateralCoin";
import type { KeyCheckName } from "../api/generated/KeyCheckName";
import type { ListFacts } from "../api/generated/ListFacts";
import type { MasternodeDetail } from "../api/generated/MasternodeDetail";
import type { MasternodeKeyInfo } from "../api/generated/MasternodeKeyInfo";
import type { MasternodeKeyRoleName } from "../api/generated/MasternodeKeyRoleName";
import type { MasternodeListState } from "../api/generated/MasternodeListState";
import type { MasternodePayout } from "../api/generated/MasternodePayout";
import type { MasternodeRow } from "../api/generated/MasternodeRow";
import type { RegistrarCapabilityName } from "../api/generated/RegistrarCapabilityName";
import type { ThemeName } from "../api/generated/ThemeName";
import type { Ctx, Handlers } from "./handlers";
import { FIXTURE_WALLET_ID, type FixtureGrant, type FixtureWallet, type World } from "./scenarios";
import { FIXTURE_NOW, secs } from "./wallet";

const COIN = 100_000_000;
const NOW = secs(FIXTURE_NOW);
const DAY = 86_400;
const HEIGHT = 1_214_880;
/** mn-engine's `MAX_MASTERNODE_LABEL`. */
export const MAX_LABEL = 64;
const HEX64 = /^[0-9a-f]{64}$/;
const SERVICE = /^(\d{1,3}(?:\.\d{1,3}){3})(?::(\d{1,5}))?$/;
/** A compressed WIF of a test network; mainnet's start with K or L, uncompressed ones with 9 or 5. */
const TEST_WIF = /^c[1-9A-HJ-NP-Za-km-z]{51}$/;

const h = (c: string) => c.repeat(Math.ceil(64 / c.length)).slice(0, 64);
const fake = (tag: string, len: number) => tag.repeat(Math.ceil(len / tag.length)).slice(0, len);
/** A fake testnet-shaped address (base58 characters, no checksum). */
const addr = (tag: string) => `y${tag.replace(/[0OIl]/g, "z")}${fake("Mn7qR2vWx9KfT4", 33 - tag.length)}`;

export const HOME = h("4e");
export const OFFICE = h("0f");
export const CLUB = h("c1");
export const OLD = h("0d");
export const FRIEND = h("f2");
export const SPARE = h("5a");

/** Fake secrets the fixture recognises (hex test vectors and Bitcoin Core's test WIF). */
export const KEY_OWNER = "11".repeat(32);
export const KEY_PAYOUT = "22".repeat(32);
export const KEY_VOTING = "33".repeat(32);
export const KEY_OTHER = "44".repeat(32);
export const TEST_VECTOR_WIF = "cVt4o7BGAig1UXywgGSmARhxMdzP5qvQsxKkSsc1XEkw3tDTQFpy";

/** The pool `mn_keys` lists per role: 25 operator keys page past 20; platform-wallet pre-derives 20 node keys. */
const POOL: Record<MasternodeKeyRoleName, number> = { owner: 40, voting: 40, operator: 25, platform_node: 20 };
const SUB: Record<MasternodeKeyRoleName, number> = { voting: 1, owner: 2, operator: 3, platform_node: 4 };

export function keyInfo(role: MasternodeKeyRoleName, index: number): MasternodeKeyInfo {
  const tag = (role.length + index).toString(16).padStart(2, "0");
  const node = role === "platform_node";
  return {
    role,
    index,
    derivationPath: `m/9'/1'/3'/${SUB[role]}'/${index}${node ? "'" : ""}`,
    address: role === "owner" || role === "voting" ? addr(`${role[0]}${index}`) : null,
    publicKeyHex: role === "operator" ? fake(`b${tag}`, 96) : fake(`02${tag}`, node ? 64 : 66),
    legacyPublicKeyHex: role === "operator" ? fake(`9${tag}`, 96) : null,
    platformNodeId: node ? fake(`d${tag}`, 40) : null,
  };
}

interface Listed {
  proTxHash: string;
  facts: ListFacts;
}

export interface MasternodesWorld {
  listState: MasternodeListState;
  /** `mn_list` fails (the list-failed notice). */
  listFails: boolean;
  /** The synced list's entries. */
  listed: Listed[];
  /** The main wallet's masternodes and the tracked ones (tracking is network-wide). */
  rows: MasternodeRow[];
  /** Masternodes whose ProRegTx a loaded wallet holds, so an owner key can be checked. */
  registrations: string[];
  /** Labels of listed masternodes that are no row. */
  labels: Record<string, string>;
  /** Payouts to the main wallet, by ProTx hash, newest first. */
  payouts: Record<string, MasternodePayout[]>;
  collateralCoins: CollateralCoin[];
  unattributed30d: number;
  /** Attached keys by ProTx hash: the secret given and its check. */
  attached: Record<string, { role: AttachedKeyRoleName; secret: string; check: KeyCheckName }[]>;
  /** The keys that match, by ProTx hash and role. */
  expected: Record<string, Partial<Record<AttachedKeyRoleName, string>>>;
  /** An evonode's claimable credits (a decimal string); absent: no identity. */
  credits: Record<string, string>;
  platformDown: boolean;
  trackedAt: Record<string, number>;
}

const fail = (code: AppError["code"], message: string, data: AppError["data"] = null): AppError => ({ code, message, data });
const copy = <T>(v: T): T => structuredClone(v);

const facts = (kind: ListFacts["kind"], n: string, valid = true, ip = `203.0.113.${n}`): ListFacts => ({
  kind,
  valid,
  service: `${ip}:19999`,
  operatorPublicKey: fake(`8${n}`, 96),
  votingAddress: addr(`v${n}`),
  platformNodeId: kind === "evonode" ? fake(`e${n}`, 40) : null,
  platformHttpPort: kind === "evonode" ? 1443 : null,
});

function payout(n: number, daysAgo: number, address: string, unattributed = false): MasternodePayout {
  return {
    txid: h(`7${n.toString(16)}`),
    recordIndex: 0,
    amount: 144_270_000,
    address,
    height: HEIGHT - Math.round(daysAgo * 576),
    timestamp: NOW - Math.round(daysAgo * DAY),
    matured: daysAgo > 0.7,
    unattributed,
  };
}

const roles = (r: Partial<MasternodeRow["roles"]> = {}): MasternodeRow["roles"] => ({ collateral: false, owner: false, voting: false, payout: false, operator: false, attached: [], ...r });

function row(proTxHash: string, label: string | null, patch: Partial<MasternodeRow>): MasternodeRow {
  return {
    proTxHash,
    kind: "regular",
    typeIndex: 1,
    label,
    status: "enabled",
    bannedSeenAt: null,
    service: null,
    mine: true,
    tracked: false,
    sharedUnsupported: false,
    shares: null,
    roles: roles(),
    collateral: { kind: "known", outpoint: `${[...proTxHash].reverse().join("")}-1` },
    collateralReleased: false,
    payoutAddress: null,
    payouts30d: 0,
    payoutsIncomplete: false,
    lastPayout: null,
    ...patch,
  };
}

const HOME_PAYOUT = addr("pH");
const OFFICE_PAYOUT = addr("pF");

/** MG-00's `mn-list` board (MG-10 §5.1). */
export function newMasternodesWorld(): MasternodesWorld {
  const home = facts("regular", "7");
  const office = facts("regular", "9", false);
  const club = facts("evonode", "21", true, "198.51.100.21");
  const homePayouts = [payout(1, 2, HOME_PAYOUT), payout(2, 9, HOME_PAYOUT), payout(3, 16, HOME_PAYOUT), payout(4, 23, HOME_PAYOUT), payout(5, 41, HOME_PAYOUT)];
  const officePayouts = [payout(6, 34, OFFICE_PAYOUT)];
  const sum = (p: MasternodePayout[]) => p.filter((x) => (x.timestamp ?? 0) >= NOW - 30 * DAY && !x.unattributed).reduce((s, x) => s + x.amount, 0);
  return {
    listState: { synced: true, height: HEIGHT },
    listFails: false,
    listed: [
      { proTxHash: HOME, facts: { ...home, operatorPublicKey: keyInfo("operator", 0).publicKeyHex } },
      { proTxHash: OFFICE, facts: { ...office, operatorPublicKey: keyInfo("operator", 1).legacyPublicKeyHex! } },
      { proTxHash: CLUB, facts: club },
      { proTxHash: FRIEND, facts: facts("evonode", "42", true, "198.51.100.42") },
      { proTxHash: SPARE, facts: facts("regular", "55", true, "192.0.2.55") },
    ],
    rows: [
      row(HOME, "Home node", {
        service: home.service,
        roles: roles({ collateral: true, owner: true, voting: true, payout: true, operator: true }),
        payoutAddress: HOME_PAYOUT,
        payouts30d: sum(homePayouts),
        lastPayout: homePayouts[0],
      }),
      row(OFFICE, "Office node", {
        typeIndex: 2,
        status: "pose_banned",
        bannedSeenAt: NOW - 24 * DAY,
        service: office.service,
        roles: roles({ collateral: true, owner: true }),
        payoutAddress: OFFICE_PAYOUT,
        lastPayout: officePayouts[0],
      }),
      row(CLUB, null, { kind: "evonode", service: club.service, mine: false, tracked: true, collateral: { kind: "unresolved" } }),
      row(OLD, "Old node", { typeIndex: 3, status: "removed", roles: roles({ collateral: true }) }),
    ],
    registrations: [HOME, OFFICE, OLD],
    labels: {},
    payouts: { [HOME]: homePayouts, [OFFICE]: officePayouts },
    collateralCoins: [],
    unattributed30d: 0,
    attached: { [CLUB]: [{ role: "owner", secret: KEY_OTHER, check: "unchecked" }] },
    expected: { [CLUB]: { owner: KEY_OWNER, payout: KEY_PAYOUT, voting: KEY_VOTING } },
    credits: { [CLUB]: "42000000000" },
    platformDown: false,
    trackedAt: { [CLUB]: NOW - 12 * DAY },
  };
}

const keyId = (secret: string) => fake(HEX64.test(secret) ? `${secret.slice(0, 2)}9` : "c0", 40);

const locked = (world: World) => world.lock.state === "locked" || world.lock.state === "unlockedMixingOnly" || world.lock.state === "noVault";

function withAttached(world: World, r: MasternodeRow): MasternodeRow {
  const keys = world.masternodes.attached[r.proTxHash] ?? [];
  const attached: AttachedKey[] | null = locked(world) ? null : keys.map((k) => ({ role: k.role, keyId: keyId(k.secret), check: k.check }));
  return { ...copy(r), roles: { ...r.roles, attached } };
}

/** The engine's order: shared → registration unavailable → keychain → vault locked → matched attached → no owner key. */
function registrar(world: World, r: MasternodeRow): RegistrarCapabilityName {
  const m = world.masternodes;
  if (r.sharedUnsupported || r.shares) return { kind: "shared" };
  if (!m.registrations.includes(r.proTxHash)) return { kind: "registration_unavailable" };
  if (r.roles.owner) return { kind: "available", signer: "keychain" };
  if (locked(world)) return { kind: "vault_locked" };
  const owner = m.attached[r.proTxHash]?.find((k) => k.role === "owner");
  return owner?.check === "matched" ? { kind: "available", signer: "attached" } : { kind: "no_owner_key" };
}

const listedOf = (m: MasternodesWorld, hash: string) => (m.listState.synced ? m.listed.find((l) => l.proTxHash === hash) : undefined);

function checkHash(hash: string) {
  if (!HEX64.test(hash)) throw fail("invalid_argument", "not a 64-character hex ProTx hash");
}

function checkLabel(label: string | null) {
  if (label !== null && label.trim().length > MAX_LABEL) throw fail("invalid_argument", `label over ${MAX_LABEL} characters`);
}

const rowsFor = (world: World, walletId: string) => world.masternodes.rows.filter((r) => r.tracked || walletId === FIXTURE_WALLET_ID);

/** The row of a masternode the wallet owns or tracks, or that the list holds; refused as `masternode` is. */
function rowOf(world: World, walletId: string, hash: string): MasternodeRow {
  checkHash(hash);
  const m = world.masternodes;
  const own = rowsFor(world, walletId).find((r) => r.proTxHash === hash);
  if (own) return own;
  const l = listedOf(m, hash);
  if (l) return row(hash, m.labels[hash] ?? null, { kind: l.facts.kind, service: l.facts.service, mine: false, status: l.facts.valid ? "enabled" : "pose_banned", collateral: { kind: "unresolved" } });
  if (!m.listState.synced) throw fail("masternodes.list_unavailable", "the masternode list is not synced");
  throw fail("masternodes.not_found", "masternode not found");
}

function tracked(m: MasternodesWorld, hash: string): MasternodeRow {
  checkHash(hash);
  const r = m.rows.find((x) => x.proTxHash === hash && x.tracked);
  if (!r) throw fail("masternodes.not_tracked", "masternode not tracked");
  return r;
}

function checkKey(world: World, r: MasternodeRow, role: AttachedKeyRoleName, key: string): KeyCheckName {
  const m = world.masternodes;
  const want = m.expected[r.proTxHash]?.[role];
  if (role === "payout") return key === want ? "matched" : "unchecked";
  if (role === "voting") {
    if (!m.listState.synced) throw fail("masternodes.list_unavailable", "the masternode list is not synced");
    if (!listedOf(m, r.proTxHash)) return "unchecked";
  } else if (!m.registrations.includes(r.proTxHash)) return "unchecked";
  if (key !== want) throw fail("masternodes.key_mismatch", `the key is not this masternode's ${role} key`, { kind: "masternodeKey", role });
  return "matched";
}

const changed = ({ world, emit }: Ctx) => emit({ kind: "masternodes", network: world.launch.network.dirName });

export function masternodesHandlers({
  wallet,
  grant,
  useGrant,
}: {
  wallet: (w: World, walletId: string) => FixtureWallet;
  grant: (w: World, id: string, walletId: string, kinds: "revealSecret"[]) => FixtureGrant | null;
  useGrant: (w: World, id: string) => void;
}): Handlers {
  return {
    mn_list_state: (_, { world }) => copy(world.masternodes.listState),
    mn_list: ({ walletId }, { world }) => {
      wallet(world, walletId);
      const m = world.masternodes;
      if (m.listFails) throw fail("internal", "fixture: the masternode list read failed");
      const rows = rowsFor(world, walletId).map((r) => withAttached(world, { ...r, mine: r.mine && walletId === FIXTURE_WALLET_ID }));
      const main = walletId === FIXTURE_WALLET_ID;
      return {
        list: copy(m.listState),
        masternodes: rows,
        collateralCoins: main ? copy(m.collateralCoins) : [],
        payouts30d: rows.filter((r) => r.mine).reduce((s, r) => s + r.payouts30d, 0),
        unattributed30d: main ? m.unattributed30d : 0,
        payoutsIncomplete: rows.some((r) => r.payoutsIncomplete),
      };
    },
    mn_masternode: ({ walletId, proTxHash }, { world }) => {
      wallet(world, walletId);
      const r = withAttached(world, rowOf(world, walletId, proTxHash));
      const m = world.masternodes;
      const held = m.registrations.includes(proTxHash);
      const detail: MasternodeDetail = {
        row: r,
        registrar: registrar(world, r),
        list: copy(listedOf(m, proTxHash)?.facts ?? null),
        registration: held
          ? {
              registeredHeight: HEIGHT - 180_000,
              collateral: copy(r.collateral),
              ownerAddress: addr("oW"),
              votingAddress: listedOf(m, proTxHash)?.facts.votingAddress ?? addr("vT"),
              payoutAddress: r.payoutAddress,
              payoutScript: r.payoutAddress ? `76a914${fake("5c", 40)}88ac` : null,
              operatorReward: 0,
              revoked: r.status === "removed",
              revocationReason: r.status === "removed" ? 0 : null,
              transactions: 1,
            }
          : null,
        payoutCheckedHeight: null,
        trackedAt: m.trackedAt[proTxHash] ?? null,
      };
      return detail;
    },
    mn_payouts: ({ walletId, proTxHash, since }, { world }) => {
      wallet(world, walletId);
      const r = rowOf(world, walletId, proTxHash);
      if (r.sharedUnsupported) throw fail("masternodes.unsupported", "shared masternodes are not supported", { kind: "unsupported", feature: "shared_masternodes", field: null });
      const all = walletId === FIXTURE_WALLET_ID ? (world.masternodes.payouts[proTxHash] ?? []) : [];
      return copy(all.filter((p) => since === null || (p.timestamp ?? 0) >= since));
    },
    mn_find: ({ query }, { world }) => {
      const m = world.masternodes;
      const q = query.trim().toLowerCase();
      const service = SERVICE.exec(q);
      if (!HEX64.test(q) && !service) throw fail("invalid_argument", "not a ProTx hash or an ip[:port]");
      if (!m.listState.synced) throw fail("masternodes.list_unavailable", "the masternode list is not synced");
      const hit = (l: Listed) => (service ? (service[2] ? l.facts.service === q : l.facts.service?.split(":")[0] === service[1]) : l.proTxHash === q);
      return m.listed.filter(hit).map((l) => ({ proTxHash: l.proTxHash, facts: copy(l.facts), tracked: m.rows.some((r) => r.proTxHash === l.proTxHash && r.tracked) }));
    },
    mn_evonode_credits: ({ proTxHash }, { world }) => {
      const m = world.masternodes;
      checkHash(proTxHash);
      if (!m.listState.synced) throw fail("masternodes.list_unavailable", "the masternode list is not synced");
      const l = listedOf(m, proTxHash);
      if (!l) throw fail("masternodes.not_found", "masternode not found");
      if (l.facts.kind !== "evonode") throw fail("masternodes.not_evonode", "not an evonode");
      if (m.platformDown) throw fail("masternodes.platform_unavailable", "Platform is unavailable");
      const credits = m.credits[proTxHash];
      if (credits === undefined) throw fail("masternodes.no_identity", "the evonode has no identity");
      return { proTxHash, credits, duffs: Number(BigInt(credits) / 1000n), fetchedAt: NOW };
    },
    mn_track: ({ proTxHash, label }, ctx) => {
      const m = ctx.world.masternodes;
      checkHash(proTxHash);
      checkLabel(label);
      if (!m.listState.synced) throw fail("masternodes.list_unavailable", "the masternode list is not synced");
      const l = listedOf(m, proTxHash);
      if (!l) throw fail("masternodes.not_found", "masternode not found");
      const existing = m.rows.find((r) => r.proTxHash === proTxHash);
      if (existing?.tracked) throw fail("masternodes.already_tracked", "already tracked");
      const name = label?.trim() || null;
      if (existing) Object.assign(existing, { tracked: true, label: name ?? existing.label });
      else {
        const typeIndex = m.rows.filter((r) => r.kind === l.facts.kind).length + 1;
        m.rows.push(row(proTxHash, name, { kind: l.facts.kind, typeIndex, service: l.facts.service, mine: false, tracked: true, status: l.facts.valid ? "enabled" : "pose_banned", collateral: { kind: "unresolved" } }));
      }
      m.trackedAt[proTxHash] = NOW;
      changed(ctx);
      return { proTxHash, facts: copy(l.facts), tracked: true };
    },
    mn_untrack: ({ proTxHash }, ctx) => {
      const { world } = ctx;
      const m = world.masternodes;
      const r = tracked(m, proTxHash);
      const keys = m.attached[proTxHash] ?? [];
      if (keys.length > 0 && locked(world)) throw fail("vault.locked", "vault locked");
      delete m.attached[proTxHash];
      delete m.trackedAt[proTxHash];
      if (r.mine) r.tracked = false;
      else m.rows = m.rows.filter((x) => x !== r);
      changed(ctx);
      return { keysDeleted: keys.length };
    },
    mn_set_label: ({ proTxHash, label }, ctx) => {
      const m = ctx.world.masternodes;
      checkLabel(label);
      const r = rowOf(ctx.world, FIXTURE_WALLET_ID, proTxHash);
      const name = label?.trim() || null;
      const kept = m.rows.find((x) => x.proTxHash === r.proTxHash);
      if (kept) kept.label = name;
      else if (name) m.labels[proTxHash] = name;
      else delete m.labels[proTxHash];
      changed(ctx);
      return null;
    },
    mn_attach_key: ({ proTxHash, role, key }, ctx) => {
      const { world } = ctx;
      checkHash(proTxHash);
      if (!HEX64.test(key) && !TEST_WIF.test(key)) throw fail("invalid_argument", "not a compressed test-network WIF or 64 hex digits");
      const r = tracked(world.masternodes, proTxHash);
      const check = checkKey(world, r, role, key);
      if (locked(world)) throw fail("vault.locked", "vault locked");
      const keys = (world.masternodes.attached[proTxHash] ??= []);
      world.masternodes.attached[proTxHash] = [...keys.filter((k) => k.role !== role), { role, secret: key, check }];
      changed(ctx);
      return check;
    },
    mn_detach_key: ({ proTxHash, role }, ctx) => {
      const { world } = ctx;
      tracked(world.masternodes, proTxHash);
      if (locked(world)) throw fail("vault.locked", "vault locked");
      const keys = world.masternodes.attached[proTxHash] ?? [];
      if (!keys.some((k) => k.role === role)) return false;
      world.masternodes.attached[proTxHash] = keys.filter((k) => k.role !== role);
      changed(ctx);
      return true;
    },
    mn_keys: ({ walletId, role, start, count }, { world }) => {
      if (count > 100) throw fail("invalid_argument", "count above 100");
      if (wallet(world, walletId).summary.watchOnly) throw fail("masternode.watch_only", "fixture: a watch-only wallet has no provider account");
      const end = Math.min(start + count, POOL[role]);
      return Array.from({ length: Math.max(0, end - start) }, (_, i) => keyInfo(role, start + i));
    },
    mn_reveal_key: ({ walletId, role, index, grant: grantId }, { world }) => {
      if (wallet(world, walletId).summary.watchOnly) throw fail("masternode.watch_only", "fixture: a watch-only wallet has no provider account");
      if (!grant(world, grantId, walletId, ["revealSecret"])) throw fail("masternode.grant_invalid", "grant invalid");
      if (index >= POOL[role]) throw fail("invalid_argument", "index past the keychain's pool");
      useGrant(world, grantId);
      const secp = role === "owner" || role === "voting";
      return {
        privateKeyHex: h((index + 1).toString(16).padStart(2, "0")),
        wif: secp ? TEST_VECTOR_WIF : null,
        tenderdashKey: role === "platform_node" ? `${"ERER".repeat(21)}EQ==` : null,
      };
    },
  };
}

// ---------------------------------------------------------------- scenarios

type Base = (theme: ThemeName | null) => World;

function mnBase(base: Base, theme: ThemeName | null, patch?: (m: MasternodesWorld, w: World) => void): World {
  const w = base(theme);
  w.settings.ui = { ...w.settings.ui, showMasternodes: true };
  patch?.(w.masternodes, w);
  return w;
}

const byHash = (m: MasternodesWorld, hash: string) => m.rows.find((r) => r.proTxHash === hash)!;

const v24 = (m: MasternodesWorld) => {
  const home = byHash(m, HOME);
  const office = byHash(m, OFFICE);
  m.payouts[HOME] = m.payouts[HOME].map((p, i) => (i % 2 === 1 ? { ...p, unattributed: true } : p));
  m.payouts[OFFICE] = [{ ...payout(6, 34, HOME_PAYOUT), unattributed: true }];
  home.payouts30d = m.payouts[HOME].filter((p) => !p.unattributed && (p.timestamp ?? 0) >= NOW - 30 * DAY).reduce((s, p) => s + p.amount, 0);
  office.payoutAddress = HOME_PAYOUT;
  office.lastPayout = m.payouts[OFFICE][0];
  for (const r of [home, office]) r.payoutsIncomplete = true;
  // Mining to another of the wallet's addresses and two v24 share payouts: counted, unmatched.
  m.unattributed30d = 210 * COIN / 100;
};

const trackOnly = (m: MasternodesWorld) => {
  m.rows = m.rows.filter((r) => r.proTxHash === CLUB);
  m.attached = {};
};

export function masternodesScenarios(base: Base) {
  return {
    /** MG-00's `mn-list`: Home (all roles), Office (banned), Club (tracked evonode, owner key attached), Old (retired). */
    "mn-list": (t: ThemeName | null) => mnBase(base, t),
    /** v24 on the network: flagged records, a shared payout address, unattributed coinbase income. */
    "mn-v24-payouts": (t: ThemeName | null) => mnBase(base, t, v24),
    "mn-empty": (t: ThemeName | null) => mnBase(base, t, (m) => ((m.rows = []), (m.payouts = {}), (m.attached = {}))),
    /** No masternode, but a 1,000 Dash output that could be a collateral. */
    "mn-empty-coin": (t: ThemeName | null) =>
      mnBase(base, t, (m) => {
        m.rows = [];
        m.payouts = {};
        m.attached = {};
        m.collateralCoins = [{ outpoint: `${h("c0")}-0`, address: addr("cL"), amount: 1_000 * COIN, kind: "regular", confirmations: 42, userLocked: false }];
      }),
    /** The list is still loading: own masternodes read Unknown. */
    "mn-syncing": (t: ThemeName | null) =>
      mnBase(base, t, (m) => {
        m.listState = { synced: false, height: null };
        for (const r of m.rows) if (r.mine) r.status = "unknown";
      }),
    "mn-list-failed": (t: ThemeName | null) => mnBase(base, t, (m) => (m.listFails = true)),
    "mn-watchonly": (t: ThemeName | null) =>
      mnBase(base, t, (_, w) => {
        w.wallets[0].summary = { ...w.wallets[0].summary, watchOnly: true };
      }),
    /** Locked vault: no attached keys shown; Club's registration is held here, so it reads `vault_locked`. */
    "mn-locked": (t: ThemeName | null) =>
      mnBase(base, t, (m, w) => {
        m.registrations.push(CLUB);
        w.lock = { ...w.lock, state: "locked" };
      }),
    /** Office's ban, with its keys here (`available`). */
    "mn-banned": (t: ThemeName | null) => mnBase(base, t, (m) => (m.rows = m.rows.filter((r) => r.proTxHash === HOME || r.proTxHash === OFFICE))),
    /** Track: Club tracked with no key, Friend's evonode and Spare node findable; no registration held for either. */
    "mn-track": (t: ThemeName | null) => mnBase(base, t, trackOnly),
    /** Track where a loaded wallet holds Club's registration: an owner key matches (`available{attached}`). */
    "mn-track-registration": (t: ThemeName | null) => mnBase(base, t, (m) => (trackOnly(m), m.registrations.push(CLUB))),
    /** `mn-track-registration`, locked: `vault_locked`, and attaching asks for the unlock. */
    "mn-track-registration-locked": (t: ThemeName | null) =>
      mnBase(base, t, (m, w) => {
        trackOnly(m);
        m.registrations.push(CLUB);
        w.lock = { ...w.lock, state: "locked" };
      }),
    /** `mn-track` locked: Club still reads `registration_unavailable`. */
    "mn-track-locked": (t: ThemeName | null) =>
      mnBase(base, t, (m, w) => {
        trackOnly(m);
        w.lock = { ...w.lock, state: "locked" };
      }),
    /** The keychain: Home's operator key matched in the basic form, Office's in the legacy form, Lab evonode's platform node id. */
    "mn-keys": (t: ThemeName | null) =>
      mnBase(base, t, (m) => {
        const lab = h("1a");
        const node = keyInfo("platform_node", 0).platformNodeId;
        m.listed.push({ proTxHash: lab, facts: { ...facts("evonode", "77", true, "198.51.100.77"), operatorPublicKey: keyInfo("operator", 2).publicKeyHex, platformNodeId: node } });
        m.rows.push(row(lab, "Lab evonode", { kind: "evonode", service: "198.51.100.77:19999", roles: roles({ collateral: true, owner: true, operator: true }) }));
        m.registrations.push(lab);
      }),
  };
}
