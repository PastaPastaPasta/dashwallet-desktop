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
import type { KeyPick } from "../api/generated/KeyPick";
import type { MasternodePayout } from "../api/generated/MasternodePayout";
import type { MnRegistrationPreview } from "../api/generated/MnRegistrationPreview";
import type { MnRegistrationRequest } from "../api/generated/MnRegistrationRequest";
import type { MasternodeTypeName } from "../api/generated/MasternodeTypeName";
import type { ProTxKeys } from "../api/generated/ProTxKeys";
import type { ProTxOperation } from "../api/generated/ProTxOperation";
import type { ProTxTerms } from "../api/generated/ProTxTerms";
import type { MasternodeRow } from "../api/generated/MasternodeRow";
import type { NetworkKind } from "../api/generated/NetworkKind";
import type { RegistrarCapabilityName } from "../api/generated/RegistrarCapabilityName";
import type { ThemeName } from "../api/generated/ThemeName";
import type { TxRow } from "../api/generated/TxRow";
import type { UpdateRegistrarPreview } from "../api/generated/UpdateRegistrarPreview";
import type { UpdateRegistrarRequest } from "../api/generated/UpdateRegistrarRequest";
import { DUP_KEY, DUP_NODE, DUP_SERVICE as DUP_SERVICE_REASON, PAYEE_REUSE } from "../masternodes/protxReasons";
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
  /** `mn_masternode` fails (a detail refresh that fails after a cached read). */
  detailFails?: boolean;
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
  /** Wallets whose seed is gone but whose provider accounts stay (Forgot Passphrase): `mn_keys`
   * lists their public keys, `mn_reveal_key` still refuses. */
  keysWithoutSeed?: string[];
  /** `mn_keys` fails for pages after the first (a keychain read that fails mid-list). */
  keyPagesFail?: boolean;
  /** A held registration's operator reward in hundredths of a percent; absent: 0. */
  operatorRewards: Record<string, number>;
  /** The wallet's ProTx operations, newest first (mn-engine §2.3). */
  operations: ProTxOperation[];
  /** Keychain roles with no unused key 0–19: `masternodes.keys_exhausted`. */
  exhausted: MasternodeKeyRoleName[];
  /** D5: the second and later previews read a higher fee. */
  previewDrift: boolean;
  previewReads: number;
  /** What a registration may spend; `null`: the main wallet's confirmed balance less the collateral coins. */
  fundingAvailable: number | null;
  /** The next `mn_register` hands off `not_sent` (`send.no_peers`), once. */
  registerNotSent: boolean;
  /** Every `needs_signature` operation's fee inputs read spent (`inputs_spent`) until it is refreshed. */
  spendInputs: boolean;
  /** Fake hashes and ids handed out so far. */
  issued: number;
  /** The next update's one hand-off finds no peers: it is saved `not_sent`. */
  nextUpdateNotSent: boolean;
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
    operations: [],
    exhausted: [],
    previewDrift: false,
    previewReads: 0,
    fundingAvailable: null,
    registerNotSent: false,
    spendInputs: false,
    issued: 0,
    nextUpdateNotSent: false,
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
  const pending = m.operations.find((u) => u.proTxHash === hash && (u.state.kind === "sent" || u.state.kind === "signed" || u.state.kind === "not_sent"));
  if (pending) throw fail("masternodes.update_pending", `operation ${pending.id} on this masternode is not in the list yet`, { kind: "operation", operation: pending.id });
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

// ---------------------------------------------------------------- ProTx owner flows (mn-engine §2.3)

/** The preview's fee by type, and what the second and later previews read with `previewDrift` (D5). */
export const REG_FEE: Record<MasternodeTypeName, number> = { regular: 453, evonode: 567 };
export const REG_FEE_DRIFT = 120;
/** Inputs the fixture refuses as Core would (MG-10 §5.1 `reg-*`). */
export const DUP_SERVICE = "203.0.113.66:19999";
export const DUP_OPERATOR_KEY = "dd".repeat(48);
/** An operator key a pending operation of the wallet names: `protx-dup` with that operation. */
export const PENDING_DUP_OPERATOR_KEY = "ee".repeat(48);
export const DUP_NODE_ID = "dd".repeat(20);
/** The one signature by an External collateral's key the fixture accepts (65 bytes of base64). */
export const EXTERNAL_SIGNATURE = `H${"A".repeat(86)}=`;
/** Another wallet's 1,000 Dash coin and its address, for External. */
export const EXTERNAL_OUTPOINT = `${h("e1")}-0`;
export const EXTERNAL_ADDRESS = addr("xC");
/** `reg`'s collateral coins: a free 1,000 Dash, a user-locked 1,000 Dash and a free 4,000 Dash. */
export const REG_COIN = `${h("c0")}-0`;
export const REG_COIN_LOCKED = `${h("c2")}-1`;
export const REG_COIN_EVO = `${h("c4")}-0`;
/** Operation ids (32 lower-case hex) of `reg-prepared` and `mn-ops`. */
const opId = (n: number) => fake(`0b${n.toString(16)}`, 32);
export const OP_PREPARED = opId(1);
export const OP_NEEDS_SIGNATURE = opId(2);
export const OP_INPUTS_SPENT = opId(3);
export const OP_SIGNED = opId(4);
export const OP_NOT_SENT = opId(5);
export const OP_SENT = opId(6);
export const OP_SENT_OVERDUE = opId(7);
export const OP_DISSOLVE = opId(8);
export const OP_DISSOLVE_FOREIGN = opId(9);
export const OP_UPDATE = opId(10);
export const OP_CONFLICTED = opId(11);
export const OP_CONFIRMED = opId(12);
export const OP_UPDATE_SAVED = opId(13);
/** A dissolve row's penalty: its `debit` is this plus the fee. */
export const DISSOLVE_PENALTY = 25 * COIN;

const SCAN = 20;
const OP_ID = /^[0-9a-f]{32}$/;
const OUTPOINT = /^[0-9a-fA-F]{64}-\d{1,10}$/;
const P2PKH = /^y[1-9A-HJ-NP-Za-km-z]{33}$/;
const P2SH = /^[89][1-9A-HJ-NP-Za-km-z]{33}$/;
const BLS = /^[0-9a-fA-F]{96}$/;
const NODE = /^[0-9a-fA-F]{40}$/;
const IPV4 = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})(?::(\d{1,5}))?$/;
const PENDING: ProTxOperation["state"]["kind"][] = ["needs_signature", "inputs_spent", "signed", "not_sent", "sent"];
const MAX_OPERATOR_REWARD = 10_000;

const refused = (reason: string) => fail("masternodes.protx_refused", reason, { kind: "reason", reason });
const exhaustedError = (role: MasternodeKeyRoleName) => fail("masternodes.keys_exhausted", `the ${role} keychain is exhausted`, { kind: "keyRole", role });
const stateError = (state: string) => fail("masternodes.operation_state", `the operation is ${state}`, { kind: "operationState", state });
const listUnavailable = () => fail("masternodes.list_unavailable", "the masternode list is not synced");

function issue(m: MasternodesWorld, tag: string, len: number): string {
  m.issued += 1;
  return fake(`${tag}${m.issued.toString(16).padStart(2, "0")}`, len);
}

const opsFor = (m: MasternodesWorld, walletId: string) => (walletId === FIXTURE_WALLET_ID ? m.operations : []);

/** The state read now: a prepared row whose fee inputs went reads `inputs_spent`. */
const readState = (m: MasternodesWorld, o: ProTxOperation): ProTxOperation["state"] => (m.spendInputs && o.state.kind === "needs_signature" ? { kind: "inputs_spent" } : o.state);
/** As the engine reads a row: the sign request only while it still needs a signature. */
const readOp = (m: MasternodesWorld, o: ProTxOperation): ProTxOperation => {
  const c = copy(o);
  const state = readState(m, o);
  return { ...c, state, signRequest: state.kind === "needs_signature" ? c.signRequest : null };
};

/** wallet → id → not found, as the engine reads an operation. */
function opOf(world: World, walletId: string, id: string): ProTxOperation {
  if (!OP_ID.test(id)) throw fail("invalid_argument", "an operation id is 32 lower-case hex digits");
  const o = opsFor(world.masternodes, walletId).find((x) => x.id === id);
  if (!o) throw fail("masternodes.operation_not_found", "operation not found");
  return o;
}

/** The keychain indices a masternode of the list, of the wallet or of a saved operation uses (MG-04 D13); a
 * `conflicted` operation holds none (D54). */
function usedKeys(m: MasternodesWorld, role: MasternodeKeyRoleName): Set<number> {
  const used = new Set<number>();
  const ops = m.operations.filter((o) => o.state.kind !== "conflicted").map((o) => o.terms);
  for (let i = 0; i < SCAN; i++) {
    const k = keyInfo(role, i);
    const listed = m.listState.synced ? m.listed.map((l) => l.facts) : [];
    if (role === "voting" && listed.some((f) => f.votingAddress === k.address)) used.add(i);
    if (role === "operator" && listed.some((f) => f.operatorPublicKey === k.publicKeyHex || f.operatorPublicKey === k.legacyPublicKeyHex)) used.add(i);
    if (role === "platform_node" && listed.some((f) => f.platformNodeId === k.platformNodeId)) used.add(i);
  }
  if (role === "owner") for (const i of Object.values(m.ownerKeys)) used.add(i);
  const index = { owner: "ownerIndex", voting: "votingIndex", operator: "operatorIndex", platform_node: "platformNodeIndex" } as const;
  for (const t of ops) {
    const i = t[index[role]];
    if (i !== null) used.add(i);
  }
  return used;
}

function nextUnused(m: MasternodesWorld, role: MasternodeKeyRoleName): number {
  if (m.exhausted.includes(role)) throw exhaustedError(role);
  const used = usedKeys(m, role);
  for (let i = 0; i < SCAN; i++) if (!used.has(i)) return i;
  throw exhaustedError(role);
}

function pick(role: MasternodeKeyRoleName, index: number): KeyPick {
  const k = keyInfo(role, index);
  return {
    index,
    address: k.address,
    publicKey: role === "operator" ? k.publicKeyHex : null,
    nodeId: k.platformNodeId,
  };
}

/** The keychain index a choice names, else the next unused one; exhausted either way when the role is (§5.1). */
function keychainIndex(m: MasternodesWorld, role: MasternodeKeyRoleName, given: number | null): number {
  if (m.exhausted.includes(role)) throw exhaustedError(role);
  return given ?? nextUnused(m, role);
}

const outpointOf = (u: { outpoint: { txid: string; vout: number } }) => `${u.outpoint.txid}-${u.outpoint.vout}`;

/** `build::check_request`: what the request's text alone refuses, before the list is read. */
function checkRequest(world: World, r: MnRegistrationRequest) {
  if ((r.kind === "evonode") !== (r.evonode !== null)) throw fail("invalid_argument", "evonode terms are given exactly for an evonode");
  if (r.operatorReward > MAX_OPERATOR_REWARD) throw fail("invalid_argument", "the operator reward is at most 10000 basis points");
  serviceOf(world, r.service);
  const c = r.collateral;
  if (c.kind !== "fund_new" && !OUTPOINT.test(c.outpoint.trim())) throw fail("invalid_argument", "not an outpoint (txid-n)");
  if (c.kind === "external" && !P2PKH.test(c.address.trim())) throw fail("invalid_argument", "the collateral address is not a P2PKH address");
  if (r.voting.kind === "address" && !P2PKH.test(r.voting.address.trim())) throw fail("invalid_argument", "the voting key is not a P2PKH address");
  if (r.operator.kind === "given" && !BLS.test(r.operator.publicKey.trim())) throw fail("invalid_argument", "the operator key is not 96 hex digits");
  if (r.evonode?.node.kind === "given" && !NODE.test(r.evonode.node.nodeId.trim())) throw fail("invalid_argument", "the platform node id is not 40 hex digits");
  if (r.payout.kind === "address" && !P2PKH.test(r.payout.address.trim()) && !P2SH.test(r.payout.address.trim())) throw fail("invalid_argument", "the payout is not a P2PKH or P2SH address");
}

/** An IPv4 service, the network's port when none is given; another network's port is Core's `bad-protx-netinfo-port`. */
function serviceOf(world: World, text: string): string {
  const s = text.trim();
  if (s === "") throw fail("invalid_argument", "a service address is required");
  const m = IPV4.exec(s);
  if (!m || m.slice(1, 5).some((o) => Number(o) > 255)) throw fail("invalid_argument", "the service is not an IPv4 address with an optional port");
  const testnet = world.launch.network.dirName !== "mainnet";
  const port = m[5] === undefined ? (testnet ? 19999 : 9999) : Number(m[5]);
  if (port < 1 || port > 65535 || (testnet ? port === 9999 : port !== 9999)) throw refused("bad-protx-netinfo-port");
  return `${m.slice(1, 5).join(".")}:${port}`;
}

const pendingOps = (m: MasternodesWorld, except?: string) => m.operations.filter((o) => o.id !== except && PENDING.includes(readState(m, o).kind) && !(o.kind === "registration" && o.state.kind === "sent" && m.rows.some((r) => r.proTxHash === o.proTxHash && r.status !== "unconfirmed")));

/** D51: an operator key, platform node id or service a pending operation of the wallet names. */
function checkPending(m: MasternodesWorld, t: ProTxTerms, except?: string) {
  for (const o of pendingOps(m, except)) {
    const claims = o.terms.operatorPublicKey === t.operatorPublicKey || (o.kind === "registration" && (o.terms.service === t.service || (!!t.platform && o.terms.platform?.nodeId === t.platform.nodeId)));
    if (claims) throw refused("protx-dup");
  }
  if (t.operatorPublicKey === PENDING_DUP_OPERATOR_KEY) throw refused("protx-dup");
}

/** The wallet's coins and addresses, for External's "this wallet's" check. */
function walletOwns(world: World, outpoint: string, address: string): boolean {
  const main = world.wallets[0];
  const m = world.masternodes;
  const coins = [...main.coins.map(outpointOf), ...m.collateralCoins.map((c) => c.outpoint)];
  const addresses = [...main.coins.map((u) => u.address), ...m.collateralCoins.map((c) => c.address)];
  return coins.includes(outpoint) || addresses.includes(address);
}

/** The collaterals the wallet's masternodes and its operations hold (a `conflicted` one holds none, D54). */
const usedCollaterals = (m: MasternodesWorld) => [
  ...m.rows.filter((r) => r.mine && r.collateral.kind === "known").map((r) => (r.collateral.kind === "known" ? r.collateral.outpoint : "")),
  ...m.operations.filter((o) => o.state.kind !== "conflicted" && o.terms.collateralOutpoint).map((o) => o.terms.collateralOutpoint!),
];

/** `resolve_registration`: the terms, refused in the engine's order. `commit` issues the payout (and a FundNew
 * collateral) address, as register and prepare do. */
function resolve(world: World, r: MnRegistrationRequest, commit: boolean): ProTxTerms {
  const m = world.masternodes;
  const service = serviceOf(world, r.service);
  const c = r.collateral;
  const outpoint = c.kind === "fund_new" ? null : c.outpoint.trim().toLowerCase();
  if (outpoint && usedCollaterals(m).includes(outpoint)) throw fail("masternodes.collateral_unavailable", "the collateral is in use");
  if (c.kind === "external" && walletOwns(world, outpoint!, c.address.trim())) throw fail("invalid_argument", "the collateral is this wallet's: register it as an existing coin");
  const ownerIndex = keychainIndex(m, "owner", r.ownerIndex);
  const ownerAddress = keyInfo("owner", ownerIndex).address!;
  const votingIndex = r.voting.kind === "address" ? null : keychainIndex(m, "voting", r.voting.kind === "keychain" ? r.voting.index : null);
  const votingAddress = r.voting.kind === "address" ? r.voting.address.trim() : keyInfo("voting", votingIndex!).address!;
  if (votingAddress === ownerAddress) throw fail("invalid_argument", "the voting key is the owner key");
  const operatorIndex = r.operator.kind === "given" ? null : keychainIndex(m, "operator", r.operator.kind === "keychain" ? r.operator.index : null);
  const operatorPublicKey = r.operator.kind === "given" ? r.operator.publicKey.trim().toLowerCase() : keyInfo("operator", operatorIndex!).publicKeyHex;
  const node = r.evonode?.node;
  const platformNodeIndex = !node || node.kind === "given" ? null : keychainIndex(m, "platform_node", node.kind === "keychain" ? node.index : null);
  const nodeId = !node ? null : node.kind === "given" ? node.nodeId.trim().toLowerCase() : keyInfo("platform_node", platformNodeIndex!).platformNodeId!;
  const payoutAddress = r.payout.kind === "address" ? r.payout.address.trim() : commit ? addr(`pN${issue(m, "", 2)}`) : "";
  if (payoutAddress === ownerAddress || payoutAddress === votingAddress) throw refused(PAYEE_REUSE);
  const listed = m.listed.map((l) => l.facts);
  if (service === DUP_SERVICE || listed.some((f) => f.service === service)) throw refused(DUP_SERVICE_REASON);
  if (operatorPublicKey === DUP_OPERATOR_KEY || listed.some((f) => f.operatorPublicKey === operatorPublicKey)) throw refused("bad-protx-dup-key");
  if (nodeId && (nodeId === DUP_NODE_ID || listed.some((f) => f.platformNodeId === nodeId))) throw refused(DUP_NODE);
  const terms: ProTxTerms = {
    masternodeType: r.kind,
    version: 2,
    service,
    ownerAddress,
    ownerIndex,
    votingAddress,
    votingIndex,
    operatorPublicKey,
    operatorIndex,
    operatorReward: r.operatorReward,
    payoutAddress,
    collateral: c.kind,
    collateralOutpoint: outpoint,
    collateralAddress: c.kind === "external" ? c.address.trim() : c.kind === "fund_new" && commit ? addr(`cN${issue(m, "", 2)}`) : null,
    platform: nodeId ? { nodeId, p2pPort: r.evonode?.p2pPort ?? PLATFORM_PORTS[world.launch.network.kind][0], httpPort: r.evonode?.httpPort ?? PLATFORM_PORTS[world.launch.network.kind][1] } : null,
    platformNodeIndex,
  };
  checkPending(m, terms);
  return terms;
}

/** dw-protx `default_ports`: a network's Platform P2P and HTTPS ports. */
const PLATFORM_PORTS: Record<NetworkKind, [number, number]> = { mainnet: [26_656, 443], testnet: [22_000, 22_001], devnet: [22_100, 22_101], regtest: [22_200, 22_201] };

/** `existing_coin`: one of the wallet's coins of exactly the type's amount (a user-locked one too: it stays locked). */
function checkCollateral(m: MasternodesWorld, r: MnRegistrationRequest) {
  if (r.collateral.kind !== "existing_utxo") return;
  const outpoint = r.collateral.outpoint.trim().toLowerCase();
  const coin = m.collateralCoins.find((x) => x.outpoint === outpoint);
  if (!coin || coin.amount !== collateralOf(r.kind)) throw fail("masternodes.collateral_unavailable", "not an unspent coin of the collateral amount");
}

const collateralOf = (kind: MasternodeTypeName) => (kind === "evonode" ? 4_000 : 1_000) * COIN;

const feeNow = (m: MasternodesWorld, kind: MasternodeTypeName) => REG_FEE[kind] + (m.previewDrift && m.previewReads > 1 ? REG_FEE_DRIFT : 0);

/** `plan_funding`: the fee, and a FundNew collateral, from the wallet's standard coins. */
function fund(world: World, r: MnRegistrationRequest): number {
  const m = world.masternodes;
  const fee = feeNow(m, r.kind);
  const needed = fee + (r.collateral.kind === "fund_new" ? collateralOf(r.kind) : 0);
  const confirmed = world.wallets[0].summary.balances?.confirmed ?? 0;
  const available = m.fundingAvailable ?? Math.max(0, confirmed - m.collateralCoins.reduce((s, c) => s + c.amount, 0));
  if (needed > available) throw fail("send.amount_exceeds_balance", "amount exceeds balance", { kind: "duffs", needed, available });
  return fee;
}

/** Begins a `MasternodeOp` lease from `grantId`, redeeming it; returns its cap. */
function lease(world: World, walletId: string, grantId: string, useGrant: (w: World, id: string) => void): number {
  const purpose = world.grants.find((x) => x.grant.id === grantId && !x.used && x.grant.walletId === walletId)?.grant.purpose;
  if (purpose?.kind !== "masternodeOp") throw fail("platform.grant_invalid", "grant invalid");
  useGrant(world, grantId);
  return purpose.maxDuffs;
}

function charge(cap: number, amount: number) {
  if (amount > cap) throw fail("send.grant_exceeded", "grant exceeded", { kind: "spendCap", maxDuffs: cap, outflow: amount });
}

const ownPayout = (address: string) => address.startsWith(addr("pN").slice(0, 3));

/** A sent registration is in the wallet's history: its masternode reads `unconfirmed` until the list has it. */
function addRegistered(m: MasternodesWorld, o: ProTxOperation) {
  const t = o.terms;
  const hash = o.proTxHash!;
  if (o.kind !== "registration" || m.rows.some((r) => r.proTxHash === hash)) return;
  const own = t.collateral !== "external";
  const outpoint = t.collateralOutpoint ?? `${hash}-1`;
  m.rows.push(
    row(hash, null, {
      kind: t.masternodeType,
      typeIndex: m.rows.filter((r) => r.mine && r.kind === t.masternodeType).length + 1,
      status: "unconfirmed",
      service: t.service,
      roles: roles({ collateral: own, owner: true, voting: t.votingIndex !== null, payout: ownPayout(t.payoutAddress), operator: t.operatorIndex !== null }),
      collateral: { kind: "known", outpoint },
      payoutAddress: t.payoutAddress,
    }),
  );
  m.registrations.push(hash);
  if (t.ownerIndex !== null) m.ownerKeys[hash] = t.ownerIndex;
  m.collateralCoins = m.collateralCoins.filter((c) => c.outpoint !== outpoint);
}

function sendOp(m: MasternodesWorld, o: ProTxOperation, notSent: boolean) {
  o.signedAt ??= NOW;
  if (notSent) Object.assign(o, { state: { kind: "not_sent" }, lastError: "send.no_peers" });
  else {
    Object.assign(o, { state: { kind: "sent" }, sentAt: NOW, lastError: null, overdue: false });
    addRegistered(m, o);
  }
}

/** Core's `MakeSignString` shape: payout|reward|owner|voting|payload hash; a new plan, a new hash. */
const signMessage = (m: MasternodesWorld, t: ProTxTerms) => `${t.payoutAddress}|${t.operatorReward}|${t.ownerAddress}|${t.votingAddress}|${issue(m, "5e", 64)}`;

function newOp(m: MasternodesWorld, kind: ProTxOperation["kind"], terms: ProTxTerms, fee: number): ProTxOperation {
  return { id: issue(m, "0c", 32), kind, proTxHash: null, txid: null, state: { kind: "signed" }, signRequest: null, fee, debit: fee, overdue: false, createdAt: NOW, signedAt: null, sentAt: null, lastError: null, terms };
}

function protxHandlers(wallet: (w: World, walletId: string) => FixtureWallet, useGrant: (w: World, id: string) => void): Handlers {
  const list = (world: World) => {
    if (!world.masternodes.listState.synced) throw listUnavailable();
  };
  /** wallet → the request's text → the list → its terms. */
  const terms = (world: World, walletId: string, request: MnRegistrationRequest, commit: boolean) => {
    wallet(world, walletId);
    checkRequest(world, request);
    list(world);
    return resolve(world, request, commit);
  };
  return {
    mn_operations: ({ walletId }, { world }) => {
      wallet(world, walletId);
      return opsFor(world.masternodes, walletId).map((o) => readOp(world.masternodes, o));
    },
    mn_registration_keys: ({ walletId, kind }, { world }) => {
      wallet(world, walletId);
      list(world);
      const m = world.masternodes;
      const keys: ProTxKeys = {
        owner: pick("owner", nextUnused(m, "owner")),
        voting: pick("voting", nextUnused(m, "voting")),
        operator: pick("operator", nextUnused(m, "operator")),
        platformNode: kind === "evonode" ? pick("platform_node", nextUnused(m, "platform_node")) : null,
      };
      return keys;
    },
    mn_preview_registration: ({ walletId, request }, { world }) => {
      const t = terms(world, walletId, request, false);
      const m = world.masternodes;
      checkCollateral(m, request);
      m.previewReads += 1;
      const fee = fund(world, request);
      const preview: MnRegistrationPreview = { terms: t, fee, totalDebit: fee, collateralAmount: collateralOf(request.kind) };
      return preview;
    },
    mn_register: ({ walletId, request, grant }, ctx) => {
      const { world } = ctx;
      if (request.collateral.kind === "external") throw fail("invalid_argument", "an external collateral is registered with prepare_external_registration");
      const t = terms(world, walletId, request, true);
      const m = world.masternodes;
      const cap = lease(world, walletId, grant, useGrant);
      checkCollateral(m, request);
      const fee = fund(world, request);
      charge(cap, fee);
      const o = newOp(m, "registration", t, fee);
      o.proTxHash = o.txid = issue(m, "9e", 64);
      if (request.collateral.kind === "fund_new") o.terms.collateralOutpoint = `${o.txid}-1`;
      sendOp(m, o, m.registerNotSent);
      m.registerNotSent = false;
      m.operations.unshift(o);
      changed(ctx);
      return readOp(m, o);
    },
    mn_prepare_external_registration: ({ walletId, request }, ctx) => {
      const { world } = ctx;
      if (request.collateral.kind !== "external") throw fail("invalid_argument", "only an external collateral is prepared; register the others");
      const t = terms(world, walletId, request, true);
      const m = world.masternodes;
      const fee = fund(world, request);
      const o = newOp(m, "registration", t, fee);
      Object.assign(o, { state: { kind: "needs_signature" }, signRequest: { address: t.collateralAddress!, message: signMessage(m, t) } });
      m.operations.unshift(o);
      changed(ctx);
      return readOp(m, o);
    },
    mn_refresh_external_registration: ({ walletId, operation }, ctx) => {
      const { world } = ctx;
      wallet(world, walletId);
      const m = world.masternodes;
      const o = opOf(world, walletId, operation);
      const state = readState(m, o).kind;
      if (state !== "needs_signature" && state !== "inputs_spent") throw stateError(state);
      // New fee inputs: the engine replans, so the fee and what leaves the wallet move (REG_FEE_DRIFT).
      if (state === "inputs_spent") Object.assign(m, { spendInputs: false }), Object.assign(o, { fee: o.fee + REG_FEE_DRIFT, debit: o.debit + REG_FEE_DRIFT });
      Object.assign(o, { state: { kind: "needs_signature" }, signRequest: { address: o.terms.collateralAddress!, message: signMessage(m, o.terms) } });
      changed(ctx);
      return readOp(m, o);
    },
    mn_submit_external_registration: ({ walletId, operation, signature, grant }, ctx) => {
      const { world } = ctx;
      wallet(world, walletId);
      const m = world.masternodes;
      const o = opOf(world, walletId, operation);
      const state = readState(m, o).kind;
      if (state !== "needs_signature") throw stateError(state);
      if (signature.trim() !== EXTERNAL_SIGNATURE) throw fail("masternodes.signature_mismatch", "the signature is not the collateral key's over the message");
      list(world);
      checkPending(m, o.terms, o.id);
      charge(lease(world, walletId, grant, useGrant), o.fee);
      o.proTxHash = o.txid = issue(m, "9e", 64);
      sendOp(m, o, false);
      changed(ctx);
      return readOp(m, o);
    },
    mn_resend_operation: ({ walletId, operation, grant }, ctx) => {
      const { world } = ctx;
      wallet(world, walletId);
      const m = world.masternodes;
      const o = opOf(world, walletId, operation);
      const state = readState(m, o).kind;
      if (state !== "signed" && state !== "not_sent" && state !== "sent") throw stateError(state);
      // `resend_conflict` (D49): an earlier update of the masternode out and not in a block could be mined after it.
      const earlier = m.operations.find((x) => x !== o && x.kind === o.kind && o.kind === "update_registrar" && x.proTxHash === o.proTxHash && x.createdAt < o.createdAt && readState(m, x).kind === "sent");
      if (earlier) throw fail("masternodes.update_pending", `operation ${earlier.id} on this masternode is not in the list yet`, { kind: "operation", operation: earlier.id });
      charge(lease(world, walletId, grant, useGrant), o.debit);
      sendOp(m, o, false);
      changed(ctx);
      return readOp(m, o);
    },
    mn_discard_operation: ({ walletId, operation }, ctx) => {
      const { world } = ctx;
      wallet(world, walletId);
      const m = world.masternodes;
      const o = opOf(world, walletId, operation);
      const state = readState(m, o).kind;
      if (!["needs_signature", "inputs_spent", "signed", "not_sent"].includes(state) || o.sentAt !== null) throw stateError(state);
      m.operations = m.operations.filter((x) => x !== o);
      changed(ctx);
      return null;
    },
    mn_release_collateral: ({ walletId, proTxHash }, ctx) => setReleased(ctx, wallet, walletId, proTxHash, true),
    mn_lock_collateral: ({ walletId, proTxHash }, ctx) => setReleased(ctx, wallet, walletId, proTxHash, false),
  };
}

/** `set_collateral_lock`: one of the wallet's masternodes (`not_found` otherwise) with a resolved collateral. */
function setReleased(ctx: Ctx, wallet: (w: World, walletId: string) => FixtureWallet, walletId: string, proTxHash: string, released: boolean): null {
  const { world } = ctx;
  wallet(world, walletId);
  const hash = checkHash(proTxHash);
  const r = world.masternodes.rows.find((x) => x.proTxHash === hash && x.mine && walletId === FIXTURE_WALLET_ID);
  if (!r) throw fail("masternodes.not_found", "masternode not found");
  if (!r.roles.collateral || r.collateral.kind !== "known") throw fail("masternodes.collateral_unavailable", "no resolved collateral");
  r.collateralReleased = released;
  changed(ctx);
  return null;
}

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
    ...protxHandlers(wallet, useGrant),
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
      if (world.masternodes.detailFails) throw fail("internal", "fixture: the masternode detail read failed");
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
      if (wallet(world, walletId).summary.watchOnly && !world.masternodes.keysWithoutSeed?.includes(walletId))
        throw fail("masternode.watch_only", "fixture: a watch-only wallet has no provider account");
      if (start > 0 && world.masternodes.keyPagesFail) throw fail("internal", "fixture: the keychain read failed");
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
      const n = m.operations.filter((o) => o.kind === "update_registrar").length + 1;
      const notSent = m.nextUpdateNotSent;
      m.nextUpdateNotSent = false;
      const op: ProTxOperation = {
        id: issue(m, "0c", 32),
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
      m.operations.unshift(op);
      changed(ctx);
      return copy(op);
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

/** The default world's: the network's list, but no masternode, tracked one or collateral of this
 * wallet's, so Show Masternodes' Auto keeps the item hidden in scenarios that aren't about them. */
export function noMasternodesWorld(): MasternodesWorld {
  return { ...newMasternodesWorld(), rows: [], payouts: {}, attached: {}, collateralCoins: [] };
}

function mnBase(base: Base, theme: ThemeName | null, patch?: (m: MasternodesWorld, w: World) => void): World {
  const w = base(theme);
  w.masternodes = newMasternodesWorld();
  w.settings.ui = { ...w.settings.ui, masternodesItem: "on" };
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

/** `reg`: no masternode yet, three collateral-shaped coins, and the balance for a FundNew collateral of either type. */
function reg(m: MasternodesWorld, w: World) {
  Object.assign(m, { rows: [], registrations: [], payouts: {}, attached: {}, trackedAt: {}, ownerKeys: {} });
  m.collateralCoins = [
    { outpoint: REG_COIN, address: addr("cL"), amount: 1_000 * COIN, kind: "regular", confirmations: 42, userLocked: false },
    { outpoint: REG_COIN_LOCKED, address: addr("cK"), amount: 1_000 * COIN, kind: "regular", confirmations: 310, userLocked: true },
    { outpoint: REG_COIN_EVO, address: addr("cE"), amount: 4_000 * COIN, kind: "evonode", confirmations: 18, userLocked: false },
  ];
  const main = w.wallets[0];
  const b = main.summary.balances;
  const extra = 10_500 * COIN;
  if (b) main.summary = { ...main.summary, balances: { ...b, confirmed: b.confirmed + extra, total: b.total + extra } };
}

function scenarioTerms(n: number, patch: Partial<ProTxTerms> = {}): ProTxTerms {
  const i = 5 + n;
  return {
    masternodeType: "regular",
    version: 2,
    service: `198.51.100.${60 + n}:19999`,
    ownerAddress: keyInfo("owner", i).address!,
    ownerIndex: i,
    votingAddress: keyInfo("voting", i).address!,
    votingIndex: i,
    operatorPublicKey: keyInfo("operator", i).publicKeyHex,
    operatorIndex: i,
    operatorReward: 0,
    payoutAddress: addr(`pO${n}`),
    collateral: "fund_new",
    collateralOutpoint: `${h(`a${n.toString(16)}`)}-1`,
    collateralAddress: addr(`cO${n}`),
    platform: null,
    platformNodeIndex: null,
    ...patch,
  };
}

function scenarioOp(id: string, n: number, state: ProTxOperation["state"], patch: Partial<ProTxOperation> = {}): ProTxOperation {
  const txid = h(`b${n.toString(16)}`);
  const signed = state.kind !== "needs_signature" && state.kind !== "inputs_spent";
  const sent = signed && state.kind !== "signed" && state.kind !== "not_sent";
  return {
    id,
    kind: "registration",
    proTxHash: signed ? txid : null,
    txid: signed ? txid : null,
    state,
    signRequest: null,
    fee: REG_FEE.regular,
    debit: REG_FEE.regular,
    overdue: false,
    createdAt: NOW - n * 3_600,
    signedAt: signed ? NOW - n * 3_600 : null,
    sentAt: sent ? NOW - n * 3_600 + 60 : null,
    lastError: null,
    terms: scenarioTerms(n),
    ...patch,
  };
}

/** A saved External registration, its keys the next unused ones, waiting for the collateral's signature. */
function prepared(m: MasternodesWorld, state: ProTxOperation["state"]) {
  const terms = scenarioTerms(1, {
    ownerIndex: null,
    collateral: "external",
    collateralOutpoint: EXTERNAL_OUTPOINT,
    collateralAddress: EXTERNAL_ADDRESS,
  });
  for (const [role, index, address] of [["owner", "ownerIndex", "ownerAddress"], ["voting", "votingIndex", "votingAddress"]] as const) {
    terms[index] = nextUnused(m, role);
    terms[address] = keyInfo(role, terms[index]).address!;
  }
  terms.operatorIndex = nextUnused(m, "operator");
  terms.operatorPublicKey = keyInfo("operator", terms.operatorIndex).publicKeyHex;
  m.operations = [scenarioOp(OP_PREPARED, 1, state, { terms, signRequest: { address: EXTERNAL_ADDRESS, message: `${terms.payoutAddress}|0|${terms.ownerAddress}|${terms.votingAddress}|${h("5e")}` } })];
}

const external = (n: number) => scenarioTerms(n, { collateral: "external", collateralOutpoint: `${h(`e${n.toString(16)}`)}-0`, collateralAddress: addr(`xE${n}`) });
const signRequest = (n: number) => ({ address: addr(`xE${n}`), message: `${addr(`pO${n}`)}|0|${keyInfo("owner", 5 + n).address}|${keyInfo("voting", 5 + n).address}|${h(`5${n.toString(16)}`)}` });

/** Home's registrar update in `state`, saved before the scenario opens (issue 438). */
const homeUpdate = (state: ProTxOperation["state"], id = OP_UPDATE, n = 9) =>
  scenarioOp(id, n, state, {
    kind: "update_registrar",
    proTxHash: HOME,
    lastError: state.kind === "not_sent" ? "send.no_peers" : null,
    terms: scenarioTerms(n, { service: "198.51.100.7:19999", ownerAddress: keyInfo("owner", 0).address!, ownerIndex: 0, collateral: null, collateralOutpoint: null, collateralAddress: null }),
  });

/** `mn-ops`: one operation in each state and of each kind, newest first (MG-10 §5.1). */
function ops(m: MasternodesWorld) {
  const dissolve = (n: number, debit: number) => ({ kind: "dissolve" as const, proTxHash: h("5d"), debit, terms: scenarioTerms(n, { collateral: null, collateralOutpoint: null, collateralAddress: null }) });
  m.operations = [
    scenarioOp(OP_NEEDS_SIGNATURE, 1, { kind: "needs_signature" }, { terms: external(1), signRequest: signRequest(1) }),
    scenarioOp(OP_INPUTS_SPENT, 2, { kind: "inputs_spent" }, { terms: external(2), signRequest: null }),
    scenarioOp(OP_SIGNED, 3, { kind: "signed" }),
    scenarioOp(OP_NOT_SENT, 4, { kind: "not_sent" }, { lastError: "send.no_peers" }),
    scenarioOp(OP_SENT, 5, { kind: "sent" }),
    scenarioOp(OP_SENT_OVERDUE, 6, { kind: "sent" }, { overdue: true }),
    scenarioOp(OP_DISSOLVE, 7, { kind: "sent" }, { ...dissolve(7, DISSOLVE_PENALTY + REG_FEE.regular), overdue: true }),
    scenarioOp(OP_DISSOLVE_FOREIGN, 8, { kind: "sent" }, dissolve(8, 0)),
    homeUpdate({ kind: "sent" }),
    scenarioOp(OP_CONFLICTED, 10, { kind: "conflicted" }),
    scenarioOp(OP_CONFIRMED, 11, { kind: "confirmed", height: HEIGHT - 40 }),
  ];
}

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
    /** `mn-list` with a change to Home saved and not sent: Change keys opens on Send again or Discard. */
    "mn-update-saved": (t: ThemeName | null) => mnBase(base, t, (m) => (m.operations = [homeUpdate({ kind: "not_sent" })])),
    /** `mn-list` with a change to Home sent and not in the list yet: a new change is refused `update_pending`. */
    "mn-update-sent": (t: ThemeName | null) => mnBase(base, t, (m) => (m.operations = [homeUpdate({ kind: "sent" })])),
    /** `mn-update-sent` with a later change to Home saved and not sent: its Send again is refused `update_pending`. */
    "mn-update-saved-behind-sent": (t: ThemeName | null) => mnBase(base, t, (m) => (m.operations = [homeUpdate({ kind: "not_sent" }, OP_UPDATE_SAVED, 2), homeUpdate({ kind: "sent" })])),
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
    /** The registration wizard: no masternode, a free and a user-locked 1,000 Dash coin and a free 4,000 Dash coin. */
    reg: (t: ThemeName | null) => mnBase(base, t, reg),
    /** `reg`, its preview fee higher on the re-read (D5). */
    "reg-preview-changes": (t: ThemeName | null) => mnBase(base, t, (m, w) => (reg(m, w), (m.previewDrift = true))),
    "reg-exhausted-owner": (t: ThemeName | null) => mnBase(base, t, (m, w) => (reg(m, w), (m.exhausted = ["owner"]))),
    "reg-exhausted-voting": (t: ThemeName | null) => mnBase(base, t, (m, w) => (reg(m, w), (m.exhausted = ["voting"]))),
    "reg-exhausted-operator": (t: ThemeName | null) => mnBase(base, t, (m, w) => (reg(m, w), (m.exhausted = ["operator"]))),
    "reg-exhausted-platform-node": (t: ThemeName | null) => mnBase(base, t, (m, w) => (reg(m, w), (m.exhausted = ["platform_node"]))),
    /** `reg`, the first registration handed off `not_sent` (no peers). */
    "reg-not-sent": (t: ThemeName | null) => mnBase(base, t, (m, w) => (reg(m, w), (m.registerNotSent = true))),
    /** `reg` with a saved External registration waiting for the collateral's signature. */
    "reg-prepared": (t: ThemeName | null) => mnBase(base, t, (m, w) => (reg(m, w), prepared(m, { kind: "needs_signature" }))),
    /** `reg-prepared`, its fee inputs spent since. */
    "reg-prepared-spent": (t: ThemeName | null) => mnBase(base, t, (m, w) => (reg(m, w), prepared(m, { kind: "inputs_spent" }))),
    /** The `mn-list` board with an operation in every state and of every kind. */
    "mn-ops": (t: ThemeName | null) => mnBase(base, t, ops),
    /** Show Masternodes on Auto: no masternode and no collateral-sized coin, only a sent registration. */
    "mn-registration-only": (t: ThemeName | null) =>
      mnBase(base, t, (m, w) => {
        Object.assign(m, { rows: [], payouts: {}, attached: {}, collateralCoins: [] });
        m.operations = [scenarioOp(OP_SENT, 5, { kind: "sent" })];
        w.settings.ui = { ...w.settings.ui, masternodesItem: "auto" };
      }),
    /** The `mn-list` board, Home node's collateral released. */
    "mn-released": (t: ThemeName | null) => mnBase(base, t, (m) => (byHash(m, HOME).collateralReleased = true)),
  };
}
