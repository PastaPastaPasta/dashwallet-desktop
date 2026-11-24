// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The fixture's masternodes (MG-10 §5.1 over mn-engine §2.1–§2.2 and the keychain, m3-engine §2.5):
// the SPV list, the main wallet's masternodes and the tracked ones, payouts, attached keys and the
// keychain. Testnet-shaped (port 19999, `y…` addresses, documentation IPs). The commands refuse in
// the engine's order; a change `emit`s `masternodes` as the engine's notifier would. Keys are
// fixed fake test vectors: `KEY_*` (repeated hex bytes) and the WIF vector of rust-dashcore's
// `dash/src/crypto/key.rs` tests (inherited from rust-bitcoin), which dw-app's masternodes_tests.rs also uses.

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
import type { ProTxOperation } from "../api/generated/ProTxOperation";
import type { ProTxTerms } from "../api/generated/ProTxTerms";
import type { RegistrarCapabilityName } from "../api/generated/RegistrarCapabilityName";
import type { ThemeName } from "../api/generated/ThemeName";
import type { TxRow } from "../api/generated/TxRow";
import type { UpdateRegistrarPreview } from "../api/generated/UpdateRegistrarPreview";
import type { UpdateRegistrarRequest } from "../api/generated/UpdateRegistrarRequest";
import { DUP_KEY, PAYEE_REUSE } from "../masternodes/protxReasons";
import type { Ctx, Handlers } from "./handlers";
import { FIXTURE_WALLET_ID, type FixtureWallet, type World } from "./scenarios";
import { FIXTURE_NOW, secs } from "./wallet";

const COIN = 100_000_000;
const NOW = secs(FIXTURE_NOW);
const DAY = 86_400;
const HEIGHT = 1_214_880;
/** mn-engine's `MAX_MASTERNODE_LABEL`. */
export const MAX_LABEL = 64;
const HEX64 = /^[0-9a-fA-F]{64}$/;
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

/** Fake secrets the fixture recognises (repeated-byte hex and the public WIF test vector above). */
export const KEY_OWNER = "11".repeat(32);
export const KEY_PAYOUT = "22".repeat(32);
export const KEY_VOTING = "33".repeat(32);
export const KEY_OTHER = "44".repeat(32);
export const TEST_VECTOR_WIF = "cVt4o7BGAig1UXywgGSmARhxMdzP5qvQsxKkSsc1XEkw3tDTQFpy";

/** The pool `mn_keys` lists per role: 25 operator keys page past 20; platform-wallet pre-derives 20 node keys. */
const POOL: Record<MasternodeKeyRoleName, number> = { owner: 40, voting: 40, operator: 25, platform_node: 20 };
const SUB: Record<MasternodeKeyRoleName, number> = { voting: 1, owner: 2, operator: 3, platform_node: 4 };

export function keyInfo(role: MasternodeKeyRoleName, index: number): MasternodeKeyInfo {
  const tag = `${SUB[role]}${index.toString(16).padStart(2, "0")}`;
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
  /** The keychain owner key index of each own masternode with the owner role; other held registrations get their own fake owner address. */
  ownerKeys: Record<string, number>;
  /** A held registration's operator reward in hundredths of a percent; absent: 0. */
  operatorRewards: Record<string, number>;
  /** The registrar updates saved (ProUpRegTx), newest last; an unsettled one blocks the next (MG-04 D49). */
  updates: ProTxOperation[];
  /** The next update's one hand-off finds no peers: it is saved `not_sent`. */
  nextUpdateNotSent: boolean;
  /** A held registration's height; absent: 180,000 blocks below the tip. */
  registeredHeights: Record<string, number>;
}

const fail = (code: AppError["code"], message: string, data: AppError["data"] = null): AppError => ({ code, message, data });
const copy = <T>(v: T): T => structuredClone(v);

const facts = (kind: ListFacts["kind"], n: string, valid = true, ip = `203.0.113.${n}`): ListFacts => ({
  kind,
  valid,
  service: `${ip}:19999`,
  operatorPublicKey: fake(`8${n}`, 96),
  votingAddress: addr(`lv${n}`),
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

export const roles = (r: Partial<MasternodeRow["roles"]> = {}): MasternodeRow["roles"] => ({ collateral: false, owner: false, voting: false, payout: false, operator: false, attached: [], ...r });

export function row(proTxHash: string, label: string | null, patch: Partial<MasternodeRow>): MasternodeRow {
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
const OLD_PAYOUT = addr("pD");

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
      { proTxHash: HOME, facts: { ...home, operatorPublicKey: keyInfo("operator", 0).publicKeyHex, votingAddress: keyInfo("voting", 0).address! } },
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
      row(CLUB, null, { kind: "evonode", typeIndex: 0, service: club.service, mine: false, tracked: true, collateral: { kind: "unresolved" } }),
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
    ownerKeys: { [HOME]: 0, [OFFICE]: 1 },
    operatorRewards: {},
    updates: [],
    nextUpdateNotSent: false,
    registeredHeights: {},
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

/** The hash in the engine's lower-case form; either case is accepted (mn-engine §1). */
function checkHash(hash: string): string {
  if (!HEX64.test(hash)) throw fail("invalid_argument", "not a 64-character hex ProTx hash");
  return hash.toLowerCase();
}

function checkLabel(label: string | null) {
  if (label !== null && label.trim().length > MAX_LABEL) throw fail("invalid_argument", `label over ${MAX_LABEL} characters`);
}

const rowsFor = (world: World, walletId: string) => world.masternodes.rows.filter((r) => r.tracked || walletId === FIXTURE_WALLET_ID);

/** The row of a masternode the wallet owns or tracks, or that the list holds; refused as `masternode` is. */
function rowOf(world: World, walletId: string, proTxHash: string): MasternodeRow {
  const hash = checkHash(proTxHash);
  const m = world.masternodes;
  const own = rowsFor(world, walletId).find((r) => r.proTxHash === hash);
  if (own) return own;
  const l = listedOf(m, hash);
  if (l) return row(hash, m.labels[hash] ?? null, { kind: l.facts.kind, typeIndex: 0, service: l.facts.service, mine: false, status: l.facts.valid ? "enabled" : "pose_banned", collateral: { kind: "unresolved" } });
  if (!m.listState.synced) throw fail("masternodes.list_unavailable", "the masternode list is not synced");
  throw fail("masternodes.not_found", "masternode not found");
}

function tracked(m: MasternodesWorld, proTxHash: string): MasternodeRow {
  const hash = checkHash(proTxHash);
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

export const UPDATE_FEE = 452;
const BLS_HEX = /^[0-9a-fA-F]{96}$/;
const TEST_ADDRESS = /^[yz8][1-9A-HJ-NP-Za-km-z]{33}$/;
/** The lowest keychain index no fixture masternode uses (D13). */
const NEXT_UNUSED = 3;

/** An operator key in the basic serialization: a legacy keychain key re-encoded (D39). */
function basicKey(key: string): string {
  for (let i = 0; i < POOL.operator; i++) if (keyInfo("operator", i).legacyPublicKeyHex === key) return keyInfo("operator", i).publicKeyHex;
  return key;
}

function checkAddress(address: string, what: string) {
  if (!TEST_ADDRESS.test(address)) throw fail("invalid_argument", `${what} is not a test-network address`);
}

/** `preview_update_registrar`'s checks and plan, in the engine's order: the request, the list, the masternode, the
 * owner key, an update still pending (D49), then what changes. */
function planUpdate(world: World, walletId: string, request: UpdateRegistrarRequest): UpdateRegistrarPreview {
  const hash = checkHash(request.proTxHash);
  if (request.operator.kind === "given" && !BLS_HEX.test(request.operator.publicKey)) throw fail("invalid_argument", "the operator key is not 96 hex digits");
  if (request.voting.kind === "address") checkAddress(request.voting.address, "the voting address");
  if (request.payout.kind === "address") checkAddress(request.payout.address, "the payout address");
  const m = world.masternodes;
  if (!m.listState.synced) throw fail("masternodes.list_unavailable", "the masternode list is not synced");
  const r = withAttached(world, rowOf(world, walletId, hash));
  const listed = listedOf(m, hash);
  if (!listed) throw fail("masternodes.not_found", "masternode not found");
  const cap = registrar(world, r);
  if (cap.kind === "shared") throw fail("masternodes.unsupported", "shared masternodes are not supported", { kind: "unsupported", feature: "shared_masternodes", field: null });
  if (cap.kind === "vault_locked") throw fail("vault.locked", "vault locked");
  if (cap.kind !== "available") throw fail("masternodes.no_owner_key", "this wallet holds no owner key of this masternode");
  const pending = m.updates.find((u) => u.proTxHash === hash && (u.state.kind === "sent" || u.state.kind === "signed" || u.state.kind === "not_sent"));
  if (pending) throw fail("masternodes.update_pending", `operation ${pending.id} on this masternode is not in the list yet`);
  const listedOperator = basicKey(listed.facts.operatorPublicKey);
  const operatorKey = request.operator.kind === "keep" ? listedOperator : request.operator.kind === "given" ? request.operator.publicKey.toLowerCase() : keyInfo("operator", request.operator.kind === "keychain" ? request.operator.index : NEXT_UNUSED).publicKeyHex;
  const operatorIndex = request.operator.kind === "keychain" ? request.operator.index : request.operator.kind === "next_unused" ? NEXT_UNUSED : null;
  const operatorChanges = operatorKey !== listedOperator;
  const votingAddress = request.voting.kind === "keep" ? listed.facts.votingAddress : request.voting.kind === "address" ? request.voting.address : keyInfo("voting", request.voting.kind === "keychain" ? request.voting.index : NEXT_UNUSED).address!;
  const votingIndex = request.voting.kind === "keychain" ? request.voting.index : request.voting.kind === "next_unused" ? NEXT_UNUSED : null;
  const payoutAddress = request.payout.kind === "address" ? request.payout.address : "";
  const ownerAddress = hash in m.ownerKeys ? keyInfo("owner", m.ownerKeys[hash]).address! : addr(`ow${hash.slice(0, 4)}`);
  if (operatorChanges && m.listed.some((l) => l.proTxHash !== hash && basicKey(l.facts.operatorPublicKey) === operatorKey))
    throw fail("masternodes.protx_refused", DUP_KEY, { kind: "reason", reason: DUP_KEY });
  if (payoutAddress && (payoutAddress === ownerAddress || payoutAddress === votingAddress))
    throw fail("masternodes.protx_refused", PAYEE_REUSE, { kind: "reason", reason: PAYEE_REUSE });
  const terms: ProTxTerms = {
    masternodeType: listed.facts.kind,
    version: 2,
    service: listed.facts.service ?? "",
    ownerAddress,
    ownerIndex: m.ownerKeys[hash] ?? null,
    votingAddress,
    votingIndex,
    operatorPublicKey: operatorKey,
    operatorIndex,
    operatorReward: m.operatorRewards[hash] ?? 0,
    payoutAddress,
    collateral: null,
    collateralOutpoint: null,
    collateralAddress: null,
    platform: null,
    platformNodeIndex: null,
  };
  return {
    terms,
    version: 2,
    fee: UPDATE_FEE,
    totalDebit: UPDATE_FEE,
    ownerKey: cap.signer === "keychain" ? { kind: "keychain", index: m.ownerKeys[hash] ?? 0 } : { kind: "attached" },
    operatorChanges,
    votingChanges: votingAddress !== listed.facts.votingAddress,
    payoutChanges: r.payoutAddress === null ? null : request.payout.kind === "new_address" || request.payout.address !== r.payoutAddress,
    poseBans: operatorChanges,
  };
}

const changed = ({ world, emit }: Ctx) => emit({ kind: "masternodes", network: world.launch.network.dirName });

export function masternodesHandlers({ wallet, useGrant }: { wallet: (w: World, walletId: string) => FixtureWallet; useGrant: (w: World, id: string) => void }): Handlers {
  /** `begin_lease` then the fee check: a vault grant of the wallet for a masternode operation, covering `debit`. */
  const masternodeGrant = (world: World, walletId: string, grantId: string, debit: number) => {
    const g = world.grants.find((x) => x.grant.id === grantId && !x.used && x.grant.walletId === walletId);
    if (!g || g.grant.purpose.kind !== "masternodeOp") throw fail("platform.grant_invalid", "grant invalid");
    if (g.grant.purpose.maxDuffs < debit)
      throw fail("send.grant_exceeded", "grant exceeded", { kind: "spendCap", maxDuffs: g.grant.purpose.maxDuffs, outflow: debit });
    useGrant(world, grantId);
  };
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
    mn_masternode: ({ walletId, proTxHash: given }, { world }) => {
      const proTxHash = checkHash(given);
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
              registeredHeight: m.registeredHeights[proTxHash] ?? HEIGHT - 180_000,
              collateral: copy(r.collateral),
              ownerAddress: proTxHash in m.ownerKeys ? keyInfo("owner", m.ownerKeys[proTxHash]).address : addr(`ow${proTxHash.slice(0, 4)}`),
              // The wallet's own record, whatever the sync state (reads.rs:596): the list's entry agrees with it.
              votingAddress: m.listed.find((l) => l.proTxHash === proTxHash)?.facts.votingAddress ?? addr(`rv${proTxHash.slice(0, 4)}`),
              payoutAddress: r.payoutAddress,
              payoutScript: r.payoutAddress ? `76a914${fake("5c", 40)}88ac` : null,
              operatorReward: m.operatorRewards[proTxHash] ?? 0,
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
    mn_payouts: ({ walletId, proTxHash: given, since }, { world }) => {
      const proTxHash = checkHash(given);
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
    mn_evonode_credits: ({ proTxHash: given }, { world }) => {
      const m = world.masternodes;
      const proTxHash = checkHash(given);
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
      const hash = checkHash(proTxHash);
      checkLabel(label);
      if (!m.listState.synced) throw fail("masternodes.list_unavailable", "the masternode list is not synced");
      const l = listedOf(m, hash);
      if (!l) throw fail("masternodes.not_found", "masternode not found");
      const existing = m.rows.find((r) => r.proTxHash === hash);
      if (existing?.tracked) throw fail("masternodes.already_tracked", "already tracked");
      const name = label?.trim() || null;
      if (existing) Object.assign(existing, { tracked: true, label: name ?? existing.label });
      else {
        m.rows.push(row(hash, name, { kind: l.facts.kind, typeIndex: 0, service: l.facts.service, mine: false, tracked: true, status: l.facts.valid ? "enabled" : "pose_banned", collateral: { kind: "unresolved" } }));
      }
      m.trackedAt[hash] = NOW;
      changed(ctx);
      return { proTxHash: hash, facts: copy(l.facts), tracked: true };
    },
    mn_untrack: ({ proTxHash }, ctx) => {
      const { world } = ctx;
      const m = world.masternodes;
      const r = tracked(m, proTxHash);
      const keys = m.attached[r.proTxHash] ?? [];
      if (keys.length > 0 && locked(world)) throw fail("vault.locked", "vault locked");
      delete m.attached[r.proTxHash];
      delete m.trackedAt[r.proTxHash];
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
      else if (name) m.labels[r.proTxHash] = name;
      else delete m.labels[r.proTxHash];
      changed(ctx);
      return null;
    },
    mn_attach_key: ({ proTxHash, role, key: given }, ctx) => {
      const { world } = ctx;
      checkHash(proTxHash);
      // The engine decodes hex to bytes (tracking.rs:58-60), so either case is one key.
      const key = HEX64.test(given) ? given.toLowerCase() : given;
      if (!HEX64.test(key) && !TEST_WIF.test(key)) throw fail("invalid_argument", "not a compressed test-network WIF or 64 hex digits");
      const r = tracked(world.masternodes, proTxHash);
      const check = checkKey(world, r, role, key);
      if (locked(world)) throw fail("vault.locked", "vault locked");
      const keys = world.masternodes.attached[r.proTxHash] ?? [];
      world.masternodes.attached[r.proTxHash] = [...keys.filter((k) => k.role !== role), { role, secret: key, check }];
      changed(ctx);
      return check;
    },
    mn_detach_key: ({ proTxHash, role }, ctx) => {
      const { world } = ctx;
      const { proTxHash: hash } = tracked(world.masternodes, proTxHash);
      if (locked(world)) throw fail("vault.locked", "vault locked");
      const keys = world.masternodes.attached[hash] ?? [];
      if (!keys.some((k) => k.role === role)) return false;
      world.masternodes.attached[hash] = keys.filter((k) => k.role !== role);
      changed(ctx);
      return true;
    },
    mn_keys: ({ walletId, role, start, count }, { world }) => {
      if (count > 100) throw fail("invalid_argument", "count above 100");
      if (wallet(world, walletId).summary.watchOnly) throw fail("masternode.watch_only", "fixture: a watch-only wallet has no provider account");
      const end = Math.min(start + count, POOL[role]);
      return Array.from({ length: Math.max(0, end - start) }, (_, i) => keyInfo(role, start + i));
    },
    mn_preview_update_registrar: ({ walletId, request }, { world }) => {
      wallet(world, walletId);
      return planUpdate(world, walletId, request);
    },
    mn_update_registrar: ({ walletId, request, grant: grantId }, ctx) => {
      const { world } = ctx;
      wallet(world, walletId);
      const plan = planUpdate(world, walletId, request);
      masternodeGrant(world, walletId, grantId, plan.totalDebit);
      const m = world.masternodes;
      const n = m.updates.length + 1;
      const notSent = m.nextUpdateNotSent;
      m.nextUpdateNotSent = false;
      const op: ProTxOperation = {
        id: `op-${fake(String(n), 8)}`,
        kind: "update_registrar",
        proTxHash: request.proTxHash.toLowerCase(),
        txid: h(`u${n}`),
        state: { kind: notSent ? "not_sent" : "sent" },
        signRequest: null,
        fee: plan.fee,
        debit: plan.totalDebit,
        overdue: false,
        createdAt: NOW,
        signedAt: NOW,
        sentAt: notSent ? null : NOW,
        lastError: notSent ? "send.no_peers" : null,
        terms: { ...plan.terms, payoutAddress: plan.terms.payoutAddress || addr(`nP${n}`) },
      };
      m.updates.push(op);
      changed(ctx);
      return copy(op);
    },
    mn_operations: ({ walletId }, { world }) => {
      wallet(world, walletId);
      return walletId === FIXTURE_WALLET_ID ? copy(world.masternodes.updates) : [];
    },
    mn_resend_operation: ({ walletId, operation, grant: grantId }, ctx) => {
      const { world } = ctx;
      wallet(world, walletId);
      const op = world.masternodes.updates.find((u) => u.id === operation);
      if (!op || walletId !== FIXTURE_WALLET_ID) throw fail("masternodes.operation_not_found", "operation not found");
      if (op.state.kind !== "not_sent" && op.state.kind !== "signed") throw fail("masternodes.operation_state", `operation is ${op.state.kind}`);
      masternodeGrant(world, walletId, grantId, op.debit);
      Object.assign(op, { state: { kind: "sent" }, sentAt: NOW, lastError: null });
      changed(ctx);
      return copy(op);
    },
    mn_discard_operation: ({ walletId, operation }, ctx) => {
      const { world } = ctx;
      wallet(world, walletId);
      const m = world.masternodes;
      const op = m.updates.find((u) => u.id === operation);
      if (!op || walletId !== FIXTURE_WALLET_ID) throw fail("masternodes.operation_not_found", "operation not found");
      if (op.state.kind === "sent" || op.state.kind === "confirmed") throw fail("masternodes.operation_state", `operation is ${op.state.kind}`);
      m.updates = m.updates.filter((u) => u !== op);
      changed(ctx);
      return null;
    },
    mn_reveal_key: ({ walletId, role, index, grant: grantId }, { world }) => {
      // dw-vault vault/masternode.rs:97-102: the grant is redeemed, then the key (its own, else the vault's) is
      // needed, then the seed (none: watch-only).
      const w = wallet(world, walletId);
      const g = world.grants.find((x) => x.grant.id === grantId && !x.used && x.grant.walletId === walletId && x.grant.purpose.kind === "revealSecret");
      if (!g) throw fail("masternode.grant_invalid", "grant invalid");
      useGrant(world, grantId);
      if (!g.ownKey && locked(world)) throw fail("masternode.vault_locked", "vault locked");
      if (w.summary.watchOnly) throw fail("masternode.watch_only", "fixture: a watch-only wallet has no provider account");
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

/** The main wallet's history holds every payout as a Generated record (Activity's filters, §1.9). */
function payoutRows(m: MasternodesWorld): TxRow[] {
  const byTxid = new Map(Object.values(m.payouts).flat().map((p) => [p.txid, p]));
  return [...byTxid.values()].map((p) => ({
    txid: p.txid,
    recordIndex: 0,
    txType: "generated",
    category: "reward",
    status: { kind: "confirmed", confirmations: 101, instantLocked: false, chainLocked: true, maturesIn: null },
    timestamp: p.timestamp,
    blockHeight: p.height,
    amount: p.amount,
    fee: null,
    address: p.address,
    label: null,
    countsTowardBalance: true,
    involvesWatchOnly: false,
  }));
}

function mnBase(base: Base, theme: ThemeName | null, patch?: (m: MasternodesWorld, w: World) => void): World {
  const w = base(theme);
  w.settings.ui = { ...w.settings.ui, showMasternodes: true };
  patch?.(w.masternodes, w);
  const main = w.wallets[0];
  main.rows = [...main.rows, ...payoutRows(w.masternodes)].sort((a, b) => (b.timestamp ?? 0) - (a.timestamp ?? 0));
  return w;
}

const byHash = (m: MasternodesWorld, hash: string) => m.rows.find((r) => r.proTxHash === hash)!;

/** Once v3 is seen every record is unattributed (reads.rs:269); a row's payouts are the records to its payout
 * address, so Home and Office, which share one, list the same payments; Old's newest is past the 30-day window. */
const v24 = (m: MasternodesWorld) => {
  const [home, office, old] = [byHash(m, HOME), byHash(m, OFFICE), byHash(m, OLD)];
  const shared = [...m.payouts[HOME], payout(6, 34, HOME_PAYOUT)].map((p) => ({ ...p, unattributed: true })).sort((a, b) => (b.timestamp ?? 0) - (a.timestamp ?? 0));
  const oldPayout = { ...payout(8, 45, OLD_PAYOUT), unattributed: true };
  m.payouts = { [HOME]: shared, [OFFICE]: shared, [OLD]: [oldPayout] };
  office.payoutAddress = HOME_PAYOUT;
  Object.assign(old, { payoutAddress: OLD_PAYOUT, lastPayout: oldPayout });
  const windowed = shared.filter((p) => (p.timestamp ?? 0) >= NOW - 30 * DAY);
  for (const r of [home, office]) Object.assign(r, { payouts30d: 0, payoutsIncomplete: windowed.length > 0, lastPayout: shared[0] });
  // Every coinbase record in the window, plus 2.10 Dash mined to another of the wallet's addresses.
  m.unattributed30d = windowed.reduce((s, p) => s + p.amount, 0) + (210 * COIN) / 100;
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
    /** The list is still loading: with no entry every row reads Unknown with no ban time, and a row's service comes only
     * from a registration the wallet holds (reads.rs:277-302, 415-420). */
    "mn-syncing": (t: ThemeName | null) =>
      mnBase(base, t, (m) => {
        m.listState = { synced: false, height: null };
        for (const r of m.rows) Object.assign(r, { status: "unknown", bannedSeenAt: null, service: m.registrations.includes(r.proTxHash) ? r.service : null });
      }),
    "mn-list-failed": (t: ThemeName | null) => mnBase(base, t, (m) => (m.listFails = true)),
    "mn-watchonly": (t: ThemeName | null) =>
      mnBase(base, t, (_, w) => {
        w.wallets[0].summary = { ...w.wallets[0].summary, watchOnly: true };
      }),
    /** Unlocked for mixing only (a locked vault is under the lock screen): no attached keys shown;
     * Club's registration is held here, so it reads `vault_locked`. */
    "mn-locked": (t: ThemeName | null) =>
      mnBase(base, t, (m, w) => {
        m.registrations.push(CLUB);
        w.lock = { ...w.lock, state: "unlockedMixingOnly" };
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
    /** `mn-list`, where the next registrar update finds no peers and is saved `not_sent`. */
    "mn-update-not-sent": (t: ThemeName | null) => mnBase(base, t, (m) => (m.nextUpdateNotSent = true)),
    /** The keychain: Home's operator key matched in the basic form, Office's in the legacy form, Lab evonode's platform node id. */
    "mn-keys": (t: ThemeName | null) =>
      mnBase(base, t, (m) => {
        const lab = h("1a");
        const node = keyInfo("platform_node", 0).platformNodeId;
        m.listed.push({ proTxHash: lab, facts: { ...facts("evonode", "77", true, "198.51.100.77"), operatorPublicKey: keyInfo("operator", 2).publicKeyHex, platformNodeId: node } });
        m.rows.push(row(lab, "Lab evonode", { kind: "evonode", service: "198.51.100.77:19999", roles: roles({ collateral: true, owner: true, operator: true }) }));
        m.registrations.push(lab);
        m.ownerKeys[lab] = 2;
      }),
  };
}
