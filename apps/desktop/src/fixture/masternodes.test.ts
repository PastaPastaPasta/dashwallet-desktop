// The fixture's masternodes (masternodes.ts, MG-10 §5.1) through the generated commands: each
// scenario's records carry exactly their generated type's fields, the commands refuse in
// mn-engine §2's order, and changes send `masternodes`.

import { readFileSync } from "node:fs";
import { clearMocks } from "@tauri-apps/api/mocks";
import { listen } from "@tauri-apps/api/event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import type { AppEvent } from "../api/generated/commands";
import { installFixture } from "./index";
import type { UpdateRegistrarRequest } from "../api/generated/UpdateRegistrarRequest";
import type { MnRegistrationRequest } from "../api/generated/MnRegistrationRequest";
import {
  CLUB,
  DISSOLVE_PENALTY,
  DUP_NODE_ID,
  DUP_OPERATOR_KEY,
  DUP_SERVICE,
  EXTERNAL_ADDRESS,
  EXTERNAL_OUTPOINT,
  EXTERNAL_SIGNATURE,
  FRIEND,
  HOME,
  KEY_OTHER,
  KEY_OWNER,
  KEY_PAYOUT,
  KEY_VOTING,
  MAX_LABEL,
  OFFICE,
  OLD,
  OP_CONFIRMED,
  OP_CONFLICTED,
  OP_DISSOLVE,
  OP_DISSOLVE_FOREIGN,
  OP_INPUTS_SPENT,
  OP_NEEDS_SIGNATURE,
  OP_NOT_SENT,
  OP_PREPARED,
  OP_SENT,
  OP_SENT_OVERDUE,
  OP_SIGNED,
  OP_UPDATE,
  OP_UPDATE_SAVED,
  PENDING_DUP_OPERATOR_KEY,
  REG_COIN,
  REG_COIN_EVO,
  REG_COIN_LOCKED,
  REG_FEE,
  REG_FEE_DRIFT,
  SPARE,
  TEST_VECTOR_WIF,
  keyId,
  keyInfo,
} from "./masternodes";
import { FIXTURE_PASSPHRASE, FIXTURE_WALLET_ID as W, type ScenarioName } from "./scenarios";

const PASS = { kind: "passphrase", passphrase: FIXTURE_PASSPHRASE } as const;
const DAY = 86_400;
const SCENARIOS: ScenarioName[] = [
  "mn-list",
  "mn-v24-payouts",
  "mn-empty",
  "mn-empty-coin",
  "mn-syncing",
  "mn-list-failed",
  "mn-watchonly",
  "mn-locked",
  "mn-banned",
  "mn-track",
  "mn-track-registration",
  "mn-track-registration-locked",
  "mn-track-locked",
  "mn-track-voting",
  "mn-track-voting-locked",
  "mn-keys",
  "reg",
  "reg-preview-changes",
  "reg-exhausted-owner",
  "reg-exhausted-voting",
  "reg-exhausted-operator",
  "reg-exhausted-platform-node",
  "reg-not-sent",
  "reg-prepared",
  "reg-prepared-spent",
  "mn-ops",
  "mn-released",
  "mn-update-not-sent",
  "mn-update-saved",
  "mn-update-sent",
  "mn-update-saved-behind-sent",
];

const install = (scenario: ScenarioName) => installFixture({ scenario });
beforeEach(() => vi.stubGlobal("window", globalThis));
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

async function events() {
  const seen: AppEvent[] = [];
  await listen<AppEvent>(cmd.ENGINE_EVENT, (e) => void seen.push(e.payload));
  return seen;
}

/** The field names of a generated object type (`export type X = { a: …, b: … };`). */
function fieldsOf(name: string): string[] {
  const text = readFileSync(new URL(`../api/generated/${name}.ts`, import.meta.url), "utf8").replace(/\/\*\*[\s\S]*?\*\//g, "");
  const body = text.slice(text.indexOf("= {") + 3, text.lastIndexOf("}"));
  return [...body.matchAll(/(?:^|,)\s*(\w+):/g)].map((m) => m[1]).sort();
}

const fields = (v: object) => Object.keys(v).sort();

function expectShape(name: string, v: object) {
  expect(fields(v), name).toEqual(fieldsOf(name));
}

async function checkShapes() {
  const overview = await cmd.mnList(W);
  expectShape("MasternodeOverview", overview);
  expectShape("MasternodeListState", overview.list);
  for (const c of overview.collateralCoins) expectShape("CollateralCoin", c);
  for (const row of overview.masternodes) {
    expectShape("MasternodeRow", row);
    expectShape("MasternodeRoles", row.roles);
    for (const k of row.roles.attached ?? []) expectShape("AttachedKey", k);
    const detail = await cmd.mnMasternode(W, row.proTxHash);
    expectShape("MasternodeDetail", detail);
    expect(detail.row).toEqual(row);
    if (detail.list) expectShape("ListFacts", detail.list);
    if (detail.registration) expectShape("RegistrationFacts", detail.registration);
    for (const p of await cmd.mnPayouts(W, row.proTxHash, null)) expectShape("MasternodePayout", p);
  }
  return overview;
}

describe("fixture masternodes", () => {
  it.each(SCENARIOS)("%s: every record has its generated shape", async (scenario) => {
    install(scenario);
    expectShape("MasternodeListState", await cmd.mnListState());
    if (scenario === "mn-list-failed") {
      await expect(cmd.mnList(W)).rejects.toMatchObject({ code: "internal" });
      return;
    }
    await checkShapes();
    for (const o of await cmd.mnOperations(W)) {
      expectShape("ProTxOperation", o);
      expectShape("ProTxTerms", o.terms);
    }
    if (scenario !== "mn-syncing") for (const l of await cmd.mnFind("198.51.100.21")) expectShape("ListedMasternode", l);
  });

  it("mn-list: Home, Office banned, Club tracked with a key, Old retired", async () => {
    install("mn-list");
    const o = await cmd.mnList(W);
    const by = new Map(o.masternodes.map((r) => [r.proTxHash, r]));
    expect([...by.keys()]).toEqual([HOME, OFFICE, CLUB, OLD]);
    expect(by.get(OFFICE)).toMatchObject({ status: "pose_banned", mine: true, roles: { owner: true, collateral: true } });
    expect(by.get(OFFICE)!.bannedSeenAt).not.toBeNull();
    expect(by.get(CLUB)).toMatchObject({ kind: "evonode", typeIndex: 0, mine: false, tracked: true, collateral: { kind: "unresolved" }, roles: { attached: [{ role: "owner", check: "unchecked" }] } });
    expect(by.get(OLD)!.status).toBe("removed");
    expect(o.unattributed30d).toBe(0);
    expect(o.payouts30d).toBe(by.get(HOME)!.payouts30d);
    expect(o.payouts30d).toBe(4 * 144_270_000);
    expect((await cmd.mnMasternode(W, HOME)).registrar).toEqual({ kind: "available", signer: "keychain" });
    expect((await cmd.mnMasternode(W, CLUB)).registrar).toEqual({ kind: "registration_unavailable" });
    expect((await cmd.mnMasternode(W, OLD)).list).toBeNull();
    // Each held registration has its own owner key; Home's and Office's are the keychain's (they hold the owner role).
    const owners = await Promise.all([HOME, OFFICE, OLD].map(async (x) => (await cmd.mnMasternode(W, x)).registration!.ownerAddress));
    expect(new Set(owners).size).toBe(3);
    expect(owners.slice(0, 2)).toEqual([keyInfo("owner", 0).address, keyInfo("owner", 1).address]);
    expect((await cmd.mnPayouts(W, HOME, null)).length).toBe(5);
    const since = (await cmd.mnPayouts(W, HOME, null))[2].timestamp!;
    expect((await cmd.mnPayouts(W, HOME, since)).length).toBe(3);
  });

  it("mn-v24-payouts: every record unattributed (reads.rs:269), one list per shared address, no attributed total", async () => {
    install("mn-v24-payouts");
    const o = await cmd.mnList(W);
    const by = new Map(o.masternodes.map((r) => [r.proTxHash, r]));
    const [home, office, old] = [by.get(HOME)!, by.get(OFFICE)!, by.get(OLD)!];
    const homePayouts = await cmd.mnPayouts(W, HOME, null);
    expect(homePayouts.length).toBe(6);
    expect(homePayouts.every((p) => p.unattributed)).toBe(true);
    const times = homePayouts.map((p) => p.timestamp!);
    expect(times).toEqual([...times].sort((a, b) => b - a));
    expect(home.payoutAddress).toBe(office.payoutAddress);
    expect(await cmd.mnPayouts(W, OFFICE, null)).toEqual(homePayouts);
    expect([home.payouts30d, office.payouts30d, o.payouts30d]).toEqual([0, 0, 0]);
    expect(home.payoutsIncomplete && office.payoutsIncomplete).toBe(true);
    expect(home.lastPayout).toEqual(office.lastPayout);
    // Every windowed coinbase record plus 2.10 Dash mined elsewhere in the wallet.
    expect(o.unattributed30d).toBe(4 * 144_270_000 + 210_000_000);
    // Old's newest payment is unattributed and past the window: no paid meta, no incomplete flag.
    const fixtureNow = Math.floor(new Date(2026, 9, 6, 12).getTime() / 1000);
    expect(old.lastPayout).toMatchObject({ unattributed: true });
    expect(old.lastPayout!.timestamp!).toBeLessThan(fixtureNow - 30 * DAY);
    expect(old.payoutsIncomplete).toBe(false);
  });

  it("mn-empty and mn-empty-coin: no rows, with and without a collateral-shaped coin", async () => {
    install("mn-empty");
    expect(await cmd.mnList(W)).toMatchObject({ masternodes: [], collateralCoins: [] });
    install("mn-empty-coin");
    const o = await cmd.mnList(W);
    expect(o.masternodes).toEqual([]);
    expect(o.collateralCoins).toMatchObject([{ amount: 1_000 * 100_000_000, userLocked: false }]);
  });

  it("mn-syncing: every row Unknown with no ban time, service only from a held registration (reads.rs:277-302, 415-420)", async () => {
    install("mn-syncing");
    expect(await cmd.mnListState()).toEqual({ synced: false, height: null });
    const o = await cmd.mnList(W);
    const by = new Map(o.masternodes.map((r) => [r.proTxHash, r]));
    expect(o.masternodes.every((r) => r.status === "unknown" && r.bannedSeenAt === null)).toBe(true);
    expect(by.get(CLUB)!.service).toBeNull();
    expect(by.get(HOME)!.service).toBe("203.0.113.7:19999");
    // A held registration's voting address is the wallet's own record, so it doesn't move with sync (reads.rs:596).
    const voting = await Promise.all([HOME, OFFICE, OLD].map(async (x) => (await cmd.mnMasternode(W, x)).registration!.votingAddress));
    expect(voting[0]).toBe(keyInfo("voting", 0).address);
    expect(new Set(voting).size).toBe(3);
    await expect(cmd.mnFind(HOME)).rejects.toMatchObject({ code: "masternodes.list_unavailable" });
    await expect(cmd.mnMasternode(W, FRIEND)).rejects.toMatchObject({ code: "masternodes.list_unavailable" });
    await expect(cmd.mnTrack(FRIEND, null)).rejects.toMatchObject({ code: "masternodes.list_unavailable" });
    await expect(cmd.mnAttachKey(CLUB, "voting", KEY_VOTING)).rejects.toMatchObject({ code: "masternodes.list_unavailable" });
  });

  it("mn-watchonly: the keychain refuses watch_only, a reveal only after its grant (dw-vault vault/masternode.rs:97-102)", async () => {
    install("mn-watchonly");
    await expect(cmd.mnKeys(W, "owner", 0, 20)).rejects.toMatchObject({ code: "masternode.watch_only" });
    await expect(cmd.mnRevealKey(W, "owner", 0, "g")).rejects.toMatchObject({ code: "masternode.grant_invalid" });
    const grant = (await cmd.authorize({ kind: "revealSecret" }, W, PASS)).id;
    await expect(cmd.mnRevealKey(W, "owner", 0, grant)).rejects.toMatchObject({ code: "masternode.watch_only" });
  });

  it("mn-locked: no attached keys, Club vault_locked, and key changes need the vault", async () => {
    install("mn-locked");
    const o = await cmd.mnList(W);
    expect(o.masternodes.every((r) => r.roles.attached === null)).toBe(true);
    expect((await cmd.mnMasternode(W, CLUB)).registrar).toEqual({ kind: "vault_locked" });
    expect((await cmd.mnMasternode(W, HOME)).registrar).toEqual({ kind: "available", signer: "keychain" });
    await expect(cmd.mnAttachKey(CLUB, "owner", KEY_OWNER)).rejects.toMatchObject({ code: "vault.locked" });
    await expect(cmd.mnDetachKey(CLUB, "owner")).rejects.toMatchObject({ code: "vault.locked" });
    await expect(cmd.mnUntrack(CLUB)).rejects.toMatchObject({ code: "vault.locked" });
  });

  it("mn-banned: Office's keys are here, so its registrar is available", async () => {
    install("mn-banned");
    expect((await cmd.mnList(W)).masternodes.map((r) => r.proTxHash)).toEqual([HOME, OFFICE]);
    expect((await cmd.mnMasternode(W, OFFICE)).registrar).toEqual({ kind: "available", signer: "keychain" });
  });
});

describe("fixture masternodes: track and keys", () => {
  it("finds by hash, ip and ip:port, refusing anything else", async () => {
    install("mn-track");
    expect((await cmd.mnFind(FRIEND)).map((l) => [l.proTxHash, l.tracked])).toEqual([[FRIEND, false]]);
    expect((await cmd.mnFind("198.51.100.21")).map((l) => [l.proTxHash, l.tracked])).toEqual([[CLUB, true]]);
    expect((await cmd.mnFind("192.0.2.55:19999")).map((l) => l.facts.kind)).toEqual(["regular"]);
    expect(await cmd.mnFind("192.0.2.55:9999")).toEqual([]);
    for (const q of ["home", "4e4e", "cVt4o7"]) await expect(cmd.mnFind(q)).rejects.toMatchObject({ code: "invalid_argument" });
  });

  it("tracks in the contract's order and sends the event", async () => {
    install("mn-track");
    const seen = await events();
    await expect(cmd.mnTrack("xyz", "a".repeat(MAX_LABEL + 1))).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(cmd.mnTrack(FRIEND, "a".repeat(MAX_LABEL + 1))).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(cmd.mnTrack(OLD, null)).rejects.toMatchObject({ code: "masternodes.not_found" });
    await expect(cmd.mnTrack(CLUB, null)).rejects.toMatchObject({ code: "masternodes.already_tracked" });
    expect(seen).toEqual([]);
    expect(await cmd.mnTrack(FRIEND, "  Friend's evonode  ")).toMatchObject({ proTxHash: FRIEND, tracked: true });
    expect(seen).toEqual([{ kind: "masternodes", network: "testnet" }]);
    const row = (await cmd.mnList(W)).masternodes.find((r) => r.proTxHash === FRIEND)!;
    expect(row).toMatchObject({ label: "Friend's evonode", kind: "evonode", tracked: true, mine: false, typeIndex: 0 });
    expect((await cmd.mnMasternode(W, FRIEND)).trackedAt).not.toBeNull();
  });

  it("checks each attached key by role and never echoes it", async () => {
    install("mn-track");
    await cmd.mnTrack(SPARE, null);
    await cmd.mnTrack(FRIEND, null);
    expect(await cmd.mnAttachKey(CLUB, "payout", KEY_PAYOUT)).toBe("matched");
    // An attach never replaces a key (tracking.rs `key_exists`): Remove comes first.
    await expect(cmd.mnAttachKey(CLUB, "payout", KEY_OTHER)).rejects.toMatchObject({ code: "masternodes.key_exists", data: { kind: "masternodeKey", role: "payout" } });
    expect(await cmd.mnDetachKey(CLUB, "payout")).toBe(true);
    expect(await cmd.mnAttachKey(CLUB, "payout", KEY_OTHER)).toBe("unchecked");
    expect(await cmd.mnAttachKey(CLUB, "voting", KEY_VOTING)).toBe("matched");
    expect(await cmd.mnAttachKey(FRIEND, "owner", KEY_OWNER)).toBe("unchecked");
    expect(await cmd.mnAttachKey(SPARE, "owner", TEST_VECTOR_WIF)).toBe("unchecked");
    for (const [hash, role, key, code] of [
      ["4e", "owner", KEY_OWNER, "invalid_argument"],
      [CLUB, "owner", "not a key", "invalid_argument"],
      [CLUB, "owner", KEY_OWNER.toUpperCase().slice(2), "invalid_argument"],
      [HOME, "owner", TEST_VECTOR_WIF, "masternodes.not_tracked"],
      [CLUB, "voting", TEST_VECTOR_WIF, "masternodes.key_exists"],
      [FRIEND, "voting", TEST_VECTOR_WIF, "masternodes.key_mismatch"],
    ] as const) {
      const e = await cmd.mnAttachKey(hash, role, key).catch((x: cmd.AppError) => x);
      expect(e, `${hash} ${role}`).toMatchObject({ code });
      expect(JSON.stringify(e)).not.toContain(key === KEY_OWNER ? "nothing" : key);
    }
    const club = (await cmd.mnList(W)).masternodes.find((r) => r.proTxHash === CLUB)!;
    expect(club.roles.attached!.map((k) => [k.role, k.check])).toEqual([
      ["payout", "unchecked"],
      ["voting", "matched"],
    ]);
    expect(await cmd.mnDetachKey(CLUB, "owner")).toBe(false);
    expect(await cmd.mnDetachKey(CLUB, "voting")).toBe(true);
    expect(await cmd.mnUntrack(CLUB)).toEqual({ keysDeleted: 1 });
    await expect(cmd.mnUntrack(CLUB)).rejects.toMatchObject({ code: "masternodes.not_tracked" });
  });

  it("mn-track-voting: Club's added voting key matches, so it holds the voting role (records.rs `holds`)", async () => {
    install("mn-track-voting");
    const club = (await cmd.mnList(W)).masternodes.find((r) => r.proTxHash === CLUB)!;
    expect(club.roles).toMatchObject({ voting: true, attached: [{ role: "voting", check: "matched" }] });
    expect(await cmd.mnDetachKey(CLUB, "voting")).toBe(true);
    expect((await cmd.mnList(W)).masternodes.find((r) => r.proTxHash === CLUB)!.roles.voting).toBe(false);
  });

  it("reads an attached voting key as unchecked while the list has no entry for its masternode", async () => {
    const f = install("mn-track-voting");
    f.world.masternodes.listed = f.world.masternodes.listed.filter((l) => l.proTxHash !== CLUB);
    const club = (await cmd.mnList(W)).masternodes.find((r) => r.proTxHash === CLUB)!;
    expect(club.roles).toMatchObject({ voting: false, attached: [{ role: "voting", check: "unchecked" }] });
  });

  it("refuses an attach while locked or unlocked for mixing only before checking it, keeping the key attached", async () => {
    const f = install("mn-track-voting-locked");
    await expect(cmd.mnAttachKey(CLUB, "voting", KEY_OTHER)).rejects.toMatchObject({ code: "vault.locked" });
    expect(f.world.masternodes.attached[CLUB]).toEqual([{ role: "voting", keyId: keyId(KEY_VOTING), check: "matched" }]);
    // The world holds key ids only: no attached secret is kept, here or across a relaunch.
    expect(JSON.stringify(f.world.masternodes).toLowerCase()).not.toContain(KEY_VOTING);
  });

  it("gives an attached voting key back through no read: list, detail, find, vote and username voters", async () => {
    install("mn-track");
    expect(await cmd.mnAttachKey(CLUB, "voting", KEY_VOTING.toUpperCase())).toBe("matched");
    const reads = await Promise.all(
      [
        () => cmd.mnList(W),
        () => cmd.mnMasternode(W, CLUB),
        () => cmd.mnFind("198.51.100.21"),
        () => cmd.governanceVotingMasternodes(W, "ab".repeat(32)),
        () => cmd.governanceContestedNames(W),
      ].map((read) => read().catch((e: unknown) => e)),
    );
    const text = JSON.stringify(reads).toLowerCase();
    expect(text).toContain(CLUB);
    expect(text).not.toContain(KEY_VOTING);
  });

  it("matches an owner key only where a loaded wallet holds the registration", async () => {
    install("mn-track-registration");
    expect((await cmd.mnMasternode(W, CLUB)).registrar).toEqual({ kind: "no_owner_key" });
    await expect(cmd.mnAttachKey(CLUB, "owner", KEY_OTHER)).rejects.toMatchObject({ code: "masternodes.key_mismatch", data: { kind: "masternodeKey", role: "owner" } });
    expect(await cmd.mnAttachKey(CLUB, "owner", KEY_OWNER)).toBe("matched");
    expect((await cmd.mnMasternode(W, CLUB)).registrar).toEqual({ kind: "available", signer: "attached" });
    install("mn-track-registration-locked");
    expect((await cmd.mnMasternode(W, CLUB)).registrar).toEqual({ kind: "vault_locked" });
    install("mn-track-locked");
    expect((await cmd.mnMasternode(W, CLUB)).registrar).toEqual({ kind: "registration_unavailable" });
  });

  it("labels a listed masternode that is no row, and clears it", async () => {
    install("mn-track");
    await expect(cmd.mnSetLabel(SPARE, "x".repeat(MAX_LABEL + 1))).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(cmd.mnSetLabel(OLD, "Old")).rejects.toMatchObject({ code: "masternodes.not_found" });
    await cmd.mnSetLabel(SPARE, " Spare ");
    expect((await cmd.mnMasternode(W, SPARE)).row.label).toBe("Spare");
    await cmd.mnSetLabel(SPARE, "  ");
    expect((await cmd.mnMasternode(W, SPARE)).row.label).toBeNull();
  });

  it("checks the hash before the wallet (reads.rs:797, 815) and takes either case (mn-engine §1)", async () => {
    install("mn-list");
    const nobody = "ef".repeat(32);
    for (const read of [() => cmd.mnMasternode(nobody, "zz"), () => cmd.mnPayouts(nobody, "zz", null)]) await expect(read()).rejects.toMatchObject({ code: "invalid_argument" });
    for (const read of [() => cmd.mnMasternode(nobody, HOME), () => cmd.mnPayouts(nobody, HOME, null)]) await expect(read()).rejects.toMatchObject({ code: "wallet_not_found" });
    expect((await cmd.mnMasternode(W, HOME.toUpperCase())).row.proTxHash).toBe(HOME);
    expect((await cmd.mnPayouts(W, HOME.toUpperCase(), null)).length).toBe(5);
    expect(await cmd.mnTrack(FRIEND.toUpperCase(), null)).toMatchObject({ proTxHash: FRIEND });
    await expect(cmd.mnTrack(FRIEND, null)).rejects.toMatchObject({ code: "masternodes.already_tracked" });
    expect(await cmd.mnAttachKey(CLUB.toUpperCase(), "payout", KEY_PAYOUT.toUpperCase())).toBe("matched");
    expect(await cmd.mnDetachKey(CLUB.toUpperCase(), "payout")).toBe(true);
    // A hex key in either case is one key, stored and shown in lower case.
    expect(await cmd.mnAttachKey(CLUB, "payout", "AB".repeat(32))).toBe("unchecked");
    const upper = (await cmd.mnList(W)).masternodes.find((r) => r.proTxHash === CLUB)!.roles.attached!.find((k) => k.role === "payout")!.keyId;
    expect(await cmd.mnDetachKey(CLUB, "payout")).toBe(true);
    expect(await cmd.mnAttachKey(CLUB, "payout", "ab".repeat(32))).toBe("unchecked");
    const lower = (await cmd.mnList(W)).masternodes.find((r) => r.proTxHash === CLUB)!.roles.attached!.find((k) => k.role === "payout")!.keyId;
    expect(upper).toBe(lower);
    expect(upper).toMatch(/^[0-9a-f]{40}$/);
  });

  it("reads an evonode's credits in the contract's order", async () => {
    install("mn-list");
    expect(await cmd.mnEvonodeCredits(CLUB)).toMatchObject({ credits: "42000000000", duffs: 42_000_000 });
    await expect(cmd.mnEvonodeCredits("zz")).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(cmd.mnEvonodeCredits(OLD)).rejects.toMatchObject({ code: "masternodes.not_found" });
    await expect(cmd.mnEvonodeCredits(SPARE)).rejects.toMatchObject({ code: "masternodes.not_evonode" });
    await expect(cmd.mnEvonodeCredits(FRIEND)).rejects.toMatchObject({ code: "masternodes.no_identity" });
  });
});

describe("fixture masternodes: the keychain", () => {
  it("pages each role and matches keys to masternodes", async () => {
    install("mn-keys");
    const operator = [...(await cmd.mnKeys(W, "operator", 0, 20)), ...(await cmd.mnKeys(W, "operator", 20, 20))];
    expect(operator.map((k) => k.index)).toEqual([...Array(25).keys()]);
    for (const k of operator) expectShape("MasternodeKeyInfo", k);
    expect((await cmd.mnKeys(W, "platform_node", 0, 100)).length).toBe(20);
    expect((await cmd.mnKeys(W, "owner", 0, 1))[0]).toMatchObject({ derivationPath: "m/9'/1'/3'/2'/0", legacyPublicKeyHex: null });
    expect((await cmd.mnKeys(W, "platform_node", 3, 1))[0].derivationPath).toBe("m/9'/1'/3'/4'/3'");
    await expect(cmd.mnKeys(W, "owner", 0, 101)).rejects.toMatchObject({ code: "invalid_argument" });
    const listed = await cmd.mnFind("198.51.100.77");
    expect(listed[0].facts.platformNodeId).toBe(keyInfo("platform_node", 0).platformNodeId);
    expect((await cmd.mnMasternode(W, HOME)).list!.operatorPublicKey).toBe(operator[0].publicKeyHex);
    expect((await cmd.mnMasternode(W, OFFICE)).list!.operatorPublicKey).toBe(operator[1].legacyPublicKeyHex);
  });

  it("matches a key only where its masternode holds that role", async () => {
    install("mn-keys");
    const voting = await cmd.mnKeys(W, "voting", 0, 40);
    const owner = await cmd.mnKeys(W, "owner", 0, 40);
    const listed = await Promise.all([HOME, OFFICE, CLUB].map(async (x) => (await cmd.mnMasternode(W, x)).list!.votingAddress));
    // Only Home holds the voting role, and only its list voting address is a keychain key.
    expect(listed.map((a) => voting.findIndex((k) => k.address === a))).toEqual([0, -1, -1]);
    // Owner key i+1 and voting key i are different keys.
    for (let i = 0; i < 39; i++) expect(owner[i + 1].publicKeyHex, String(i)).not.toBe(voting[i].publicKeyHex);
    const all = [...owner, ...voting, ...(await cmd.mnKeys(W, "operator", 0, 25)), ...(await cmd.mnKeys(W, "platform_node", 0, 20))];
    expect(new Set(all.map((k) => k.publicKeyHex)).size).toBe(all.length);
  });

  it("reveals one key per revealSecret grant", async () => {
    install("mn-keys");
    await expect(cmd.mnRevealKey(W, "owner", 0, "no-such-grant")).rejects.toMatchObject({ code: "masternode.grant_invalid" });
    const grant = (await cmd.authorize({ kind: "revealSecret" }, W, PASS)).id;
    const owner = await cmd.mnRevealKey(W, "owner", 0, grant);
    expectShape("RevealedMasternodeKey", owner);
    expect(owner).toMatchObject({ wif: TEST_VECTOR_WIF, tenderdashKey: null });
    expect(owner.privateKeyHex).toMatch(/^[0-9a-f]{64}$/);
    await expect(cmd.mnRevealKey(W, "owner", 0, grant)).rejects.toMatchObject({ code: "masternode.grant_invalid" });
    const node = await cmd.mnRevealKey(W, "platform_node", 0, (await cmd.authorize({ kind: "revealSecret" }, W, PASS)).id);
    expect(node.wif).toBeNull();
    expect(node.tenderdashKey).toHaveLength(88);
    const op = await cmd.mnRevealKey(W, "operator", 4, (await cmd.authorize({ kind: "revealSecret" }, W, PASS)).id);
    expect([op.wif, op.tenderdashKey]).toEqual([null, null]);
    // No pool limits a reveal: the engine derives any index (masternode_keys.rs:293-313).
    expect((await cmd.mnRevealKey(W, "owner", 100, (await cmd.authorize({ kind: "revealSecret" }, W, PASS)).id)).wif).toBe(TEST_VECTOR_WIF);
  });

  it("needs the grant, then a key, then the seed (dw-vault vault/masternode.rs:97-102)", async () => {
    const f = installFixture({ scenario: "mn-keys" });
    const before = (await cmd.authorize({ kind: "revealSecret" }, W, PASS)).id;
    f.world.lock = { ...f.world.lock, state: "unlockedMixingOnly" };
    await expect(cmd.mnRevealKey(W, "owner", 0, before)).rejects.toMatchObject({ code: "masternode.vault_locked" });
    await expect(cmd.mnRevealKey(W, "owner", 0, before)).rejects.toMatchObject({ code: "masternode.grant_invalid" });
    // A passphrase grant issued while locked carries its own key.
    f.world.lock = { ...f.world.lock, state: "locked" };
    const own = (await cmd.authorize({ kind: "revealSecret" }, W, PASS)).id;
    expect((await cmd.mnRevealKey(W, "voting", 0, own)).wif).toBe(TEST_VECTOR_WIF);
  });
});

const request = (patch: Partial<MnRegistrationRequest> = {}): MnRegistrationRequest => ({
  kind: "regular",
  collateral: { kind: "fund_new" },
  service: "198.51.100.90:19999",
  operator: { kind: "next_unused" },
  operatorReward: 0,
  ownerIndex: null,
  voting: { kind: "next_unused" },
  payout: { kind: "new_address" },
  evonode: null,
  ...patch,
});
const evonode = (patch: Partial<MnRegistrationRequest> = {}) => request({ kind: "evonode", evonode: { node: { kind: "next_unused" }, p2pPort: null, httpPort: null }, ...patch });
const external = (patch: Partial<MnRegistrationRequest> = {}) => request({ collateral: { kind: "external", outpoint: `${"e7".repeat(32)}-0`, address: EXTERNAL_ADDRESS }, ...patch });
const mnGrant = async (maxDuffs: number) => (await cmd.authorize({ kind: "masternodeOp", maxDuffs }, W, PASS)).id;
const code = (p: Promise<unknown>) => p.then(() => "ok", (e: { code: string }) => e.code);

describe("fixture masternodes: registration", () => {
  it("reg: no masternode, three coins, the next unused keys of the list and the wallet", async () => {
    install("reg");
    const o = await cmd.mnList(W);
    expect(o.masternodes).toEqual([]);
    expect(o.collateralCoins.map((c) => [c.outpoint, c.kind, c.userLocked])).toEqual([
      [REG_COIN, "regular", false],
      [REG_COIN_LOCKED, "regular", true],
      [REG_COIN_EVO, "evonode", false],
    ]);
    const keys = await cmd.mnRegistrationKeys(W, "regular");
    expectShape("ProTxKeys", keys);
    expectShape("KeyPick", keys.owner);
    // The list holds Home's voting key 0 and operator keys 0 (basic) and 1 (legacy).
    expect([keys.owner.index, keys.voting.index, keys.operator.index, keys.platformNode]).toEqual([0, 1, 2, null]);
    expect(keys.owner.address).toBe(keyInfo("owner", 0).address);
    expect(keys.operator.publicKey).toBe(keyInfo("operator", 2).publicKeyHex);
    expect((await cmd.mnRegistrationKeys(W, "evonode")).platformNode).toMatchObject({ index: 0, nodeId: keyInfo("platform_node", 0).platformNodeId });
  });

  it.each(["owner", "voting", "operator", "platform_node"] as const)("reg-exhausted: %s refuses keys_exhausted with its role", async (role) => {
    install(`reg-exhausted-${role.replace("_", "-")}` as ScenarioName);
    const want = { code: "masternodes.keys_exhausted", data: { kind: "keyRole", role } };
    await expect(cmd.mnRegistrationKeys(W, "evonode")).rejects.toMatchObject(want);
    await expect(cmd.mnPreviewRegistration(W, evonode())).rejects.toMatchObject(want);
  });

  it("previews: the fee is the debit, FundNew's collateral stays in the wallet, the addresses are issued later", async () => {
    install("reg");
    const p = await cmd.mnPreviewRegistration(W, request());
    expectShape("MnRegistrationPreview", p);
    expect(p).toMatchObject({ fee: REG_FEE.regular, totalDebit: REG_FEE.regular, collateralAmount: 1_000 * 100_000_000 });
    expect(p.terms).toMatchObject({ payoutAddress: "", collateral: "fund_new", collateralOutpoint: null, collateralAddress: null, ownerIndex: 0, votingIndex: 1, operatorIndex: 2 });
    const evo = await cmd.mnPreviewRegistration(W, evonode({ collateral: { kind: "existing_utxo", outpoint: REG_COIN_EVO } }));
    expect(evo.terms).toMatchObject({ collateralOutpoint: REG_COIN_EVO, platform: { nodeId: keyInfo("platform_node", 0).platformNodeId, p2pPort: 22000, httpPort: 22001 } });
    // A user-locked coin is a collateral too (build.rs existing_coin); it stays locked.
    await expect(cmd.mnPreviewRegistration(W, request({ collateral: { kind: "existing_utxo", outpoint: REG_COIN_LOCKED } }))).resolves.toBeTruthy();
  });

  it("D5: reg-preview-changes reads a higher fee from the second preview on", async () => {
    install("reg-preview-changes");
    const first = await cmd.mnPreviewRegistration(W, request());
    const second = await cmd.mnPreviewRegistration(W, request());
    expect([first.fee, second.fee]).toEqual([REG_FEE.regular, REG_FEE.regular + REG_FEE_DRIFT]);
    expect(second.totalDebit).toBe(second.fee);
  });

  it("refuses in the engine's order: wallet, the request's text, the list, then the terms", async () => {
    const f = install("reg");
    await expect(cmd.mnPreviewRegistration("ff".repeat(32), request({ service: "nope" }))).rejects.toMatchObject({ code: "wallet_not_found" });
    f.world.masternodes.listState = { synced: false, height: null };
    for (const bad of [
      request({ service: "nope" }),
      request({ service: "300.1.1.1:19999" }),
      request({ operator: { kind: "given", publicKey: "ab".repeat(40) } }),
      evonode({ evonode: { node: { kind: "given", nodeId: "ab" }, p2pPort: null, httpPort: null } }),
      request({ collateral: { kind: "existing_utxo", outpoint: "abc-0" } }),
      request({ evonode: { node: { kind: "next_unused" }, p2pPort: null, httpPort: null } }),
      request({ operatorReward: 10_001 }),
    ]) {
      await expect(cmd.mnPreviewRegistration(W, bad)).rejects.toMatchObject({ code: "invalid_argument" });
    }
    await expect(cmd.mnPreviewRegistration(W, request({ service: "198.51.100.90:9999" }))).rejects.toMatchObject({ code: "masternodes.protx_refused", data: { reason: "bad-protx-netinfo-port" } });
    await expect(cmd.mnPreviewRegistration(W, request({ service: DUP_SERVICE }))).rejects.toMatchObject({ code: "masternodes.list_unavailable" });
    await expect(cmd.mnRegistrationKeys(W, "regular")).rejects.toMatchObject({ code: "masternodes.list_unavailable" });
    f.world.masternodes.listState = { synced: true, height: 1 };
    // The collateral before the keys and Core's rules; the coin check after them.
    await expect(cmd.mnPreviewRegistration(W, request({ service: DUP_SERVICE, collateral: { kind: "existing_utxo", outpoint: `${"99".repeat(32)}-0` } }))).rejects.toMatchObject({
      code: "masternodes.protx_refused",
    });
  });

  it("refuses keyed inputs as Core would", async () => {
    install("reg");
    const refused = async (r: MnRegistrationRequest) => {
      const e = await cmd.mnPreviewRegistration(W, r).catch((x: { code: string; data: { reason: string } }) => x);
      expect(e).toMatchObject({ code: "masternodes.protx_refused" });
      return (e as { data: { reason: string } }).data;
    };
    expect(await refused(request({ service: DUP_SERVICE }))).toEqual({ kind: "reason", reason: "bad-protx-dup-netinfo-entry" });
    expect((await refused(request({ service: "203.0.113.7:19999" }))).reason).toBe("bad-protx-dup-netinfo-entry");
    expect((await refused(request({ operator: { kind: "given", publicKey: DUP_OPERATOR_KEY } }))).reason).toBe("bad-protx-dup-key");
    expect((await refused(evonode({ evonode: { node: { kind: "given", nodeId: DUP_NODE_ID }, p2pPort: null, httpPort: null } }))).reason).toBe("bad-protx-dup-platformnodeid");
    expect((await refused(request({ payout: { kind: "address", address: keyInfo("owner", 0).address! } }))).reason).toBe("bad-protx-payee-reuse");
    expect((await refused(request({ operator: { kind: "given", publicKey: PENDING_DUP_OPERATOR_KEY } }))).reason).toBe("protx-dup");
    await expect(cmd.mnPreviewRegistration(W, request({ voting: { kind: "address", address: keyInfo("owner", 0).address! } }))).rejects.toMatchObject({ code: "invalid_argument" });
    for (const outpoint of [`${"99".repeat(32)}-0`, REG_COIN_EVO]) {
      await expect(cmd.mnPreviewRegistration(W, request({ collateral: { kind: "existing_utxo", outpoint } }))).rejects.toMatchObject({ code: "masternodes.collateral_unavailable" });
    }
    const own = await cmd.mnPreviewRegistration(W, external({ collateral: { kind: "external", outpoint: REG_COIN, address: EXTERNAL_ADDRESS } })).catch((e: { code: string; message: string }) => e);
    expect(own).toMatchObject({ code: "invalid_argument" });
    expect((own as { message: string }).message).toContain("collateral is this wallet's");
  });

  it("refuses a FundNew collateral the wallet can't fund, naming what it needs", async () => {
    install("mn-empty");
    await expect(cmd.mnPreviewRegistration(W, request())).rejects.toMatchObject({ code: "send.amount_exceeds_balance", data: { kind: "duffs", needed: 1_000 * 100_000_000 + REG_FEE.regular } });
  });

  it("registers under a masternodeOp grant charged the fee, and sends the event", async () => {
    install("reg");
    const seen = await events();
    await expect(cmd.mnRegister(W, external(), await mnGrant(10_000))).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(cmd.mnRegister(W, request(), "no-such-grant")).rejects.toMatchObject({ code: "platform.grant_invalid" });
    await expect(cmd.mnRegister(W, request(), (await cmd.authorize({ kind: "spend", maxDuffs: 10_000 }, W, PASS)).id)).rejects.toMatchObject({ code: "platform.grant_invalid" });
    const short = await mnGrant(REG_FEE.regular - 1);
    await expect(cmd.mnRegister(W, request(), short)).rejects.toMatchObject({ code: "send.grant_exceeded", data: { kind: "spendCap", maxDuffs: REG_FEE.regular - 1, outflow: REG_FEE.regular } });
    await expect(cmd.mnRegister(W, request(), short)).rejects.toMatchObject({ code: "platform.grant_invalid" });
    expect(seen).toEqual([]);
    const op = await cmd.mnRegister(W, request({ collateral: { kind: "existing_utxo", outpoint: REG_COIN } }), await mnGrant(REG_FEE.regular + 10_000));
    expectShape("ProTxOperation", op);
    expect(op).toMatchObject({ kind: "registration", state: { kind: "sent" }, fee: REG_FEE.regular, debit: REG_FEE.regular, overdue: false });
    expect(op.terms.payoutAddress).not.toBe("");
    expect(seen).toEqual([{ kind: "masternodes", network: "testnet" }]);
    const o = await cmd.mnList(W);
    expect(o.masternodes).toMatchObject([{ proTxHash: op.proTxHash, status: "unconfirmed", mine: true, collateralReleased: false, roles: { collateral: true, owner: true, voting: true, payout: true, operator: true } }]);
    expect(o.collateralCoins.map((c) => c.outpoint)).not.toContain(REG_COIN);
    expect(await cmd.mnOperations(W)).toEqual([op]);
    // Its keys and collateral are taken now.
    expect((await cmd.mnRegistrationKeys(W, "regular")).owner.index).toBe(1);
    await expect(cmd.mnPreviewRegistration(W, request({ collateral: { kind: "existing_utxo", outpoint: REG_COIN } }))).rejects.toMatchObject({ code: "masternodes.collateral_unavailable" });
    await expect(cmd.mnPreviewRegistration(W, request({ service: op.terms.service }))).rejects.toMatchObject({ code: "masternodes.protx_refused", data: { kind: "reason", reason: "protx-dup" } });
  });

  it("reg-not-sent: the first hand-off is not_sent, Send again sends it", async () => {
    install("reg-not-sent");
    const op = await cmd.mnRegister(W, request(), await mnGrant(20_000));
    expect(op).toMatchObject({ state: { kind: "not_sent" }, lastError: "send.no_peers", sentAt: null });
    expect((await cmd.mnList(W)).masternodes).toEqual([]);
    expect(await code(cmd.mnResendOperation(W, op.id, await mnGrant(op.debit - 1)))).toBe("send.grant_exceeded");
    const sent = await cmd.mnResendOperation(W, op.id, await mnGrant(op.debit));
    expect(sent).toMatchObject({ state: { kind: "sent" }, lastError: null });
    expect((await cmd.mnList(W)).masternodes.map((r) => r.proTxHash)).toEqual([op.proTxHash]);
  });

  it("External: prepare, then submit the collateral's signature under the grant", async () => {
    install("reg");
    const seen = await events();
    await expect(cmd.mnPrepareExternalRegistration(W, request())).rejects.toMatchObject({ code: "invalid_argument" });
    const op = await cmd.mnPrepareExternalRegistration(W, external());
    expect(op).toMatchObject({ state: { kind: "needs_signature" }, txid: null, proTxHash: null, signRequest: { address: EXTERNAL_ADDRESS } });
    expect(op.signRequest!.message).toContain(op.terms.ownerAddress);
    expect(op.terms.payoutAddress).not.toBe("");
    expect(seen).toHaveLength(1);
    await expect(cmd.mnSubmitExternalRegistration(W, "nope", EXTERNAL_SIGNATURE, await mnGrant(10_000))).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(cmd.mnSubmitExternalRegistration(W, "ab".repeat(16), EXTERNAL_SIGNATURE, await mnGrant(10_000))).rejects.toMatchObject({ code: "masternodes.operation_not_found" });
    await expect(cmd.mnSubmitExternalRegistration(W, op.id, "H" + "B".repeat(86) + "=", await mnGrant(10_000))).rejects.toMatchObject({ code: "masternodes.signature_mismatch" });
    await expect(cmd.mnSubmitExternalRegistration(W, op.id, EXTERNAL_SIGNATURE, await mnGrant(op.fee - 1))).rejects.toMatchObject({ code: "send.grant_exceeded" });
    const sent = await cmd.mnSubmitExternalRegistration(W, op.id, EXTERNAL_SIGNATURE, await mnGrant(op.debit));
    expect(sent).toMatchObject({ id: op.id, state: { kind: "sent" } });
    expect(sent.txid).toMatch(/^[0-9a-f]{64}$/);
    expect((await cmd.mnList(W)).masternodes).toMatchObject([{ proTxHash: sent.proTxHash, roles: { collateral: false, owner: true } }]);
    await expect(cmd.mnSubmitExternalRegistration(W, op.id, EXTERNAL_SIGNATURE, await mnGrant(10_000))).rejects.toMatchObject({ code: "masternodes.operation_state", data: { kind: "operationState", state: "sent" } });
    expect(seen).toHaveLength(2);
  });

  it("reg-prepared: the saved row continues; spent inputs refuse the submit until it is refreshed", async () => {
    const f = install("reg-prepared");
    const [op] = await cmd.mnOperations(W);
    expect(op).toMatchObject({ id: OP_PREPARED, state: { kind: "needs_signature" }, terms: { collateralOutpoint: EXTERNAL_OUTPOINT } });
    // Its keys are taken.
    expect((await cmd.mnRegistrationKeys(W, "regular")).owner.index).toBe(op.terms.ownerIndex! + 1);
    await expect(cmd.mnPreviewRegistration(W, external({ collateral: { kind: "external", outpoint: EXTERNAL_OUTPOINT, address: EXTERNAL_ADDRESS } }))).rejects.toMatchObject({ code: "masternodes.collateral_unavailable" });
    f.world.masternodes.spendInputs = true;
    expect((await cmd.mnOperations(W))[0].state).toEqual({ kind: "inputs_spent" });
    await expect(cmd.mnSubmitExternalRegistration(W, op.id, EXTERNAL_SIGNATURE, await mnGrant(10_000))).rejects.toMatchObject({ code: "masternodes.operation_state", data: { state: "inputs_spent" } });
    const seen = await events();
    const fresh = await cmd.mnRefreshExternalRegistration(W, op.id);
    expect(fresh.state).toEqual({ kind: "needs_signature" });
    expect(fresh.signRequest!.message).not.toBe(op.signRequest!.message);
    expect(seen).toHaveLength(1);
    expect((await cmd.mnSubmitExternalRegistration(W, op.id, EXTERNAL_SIGNATURE, await mnGrant(10_000))).state).toEqual({ kind: "sent" });
    await expect(cmd.mnRefreshExternalRegistration(W, op.id)).rejects.toMatchObject({ code: "masternodes.operation_state", data: { state: "sent" } });
  });

  it("reg-prepared-spent: refresh brings it back to needs_signature", async () => {
    install("reg-prepared-spent");
    expect((await cmd.mnOperations(W))[0].state).toEqual({ kind: "inputs_spent" });
    expect((await cmd.mnRefreshExternalRegistration(W, OP_PREPARED)).state).toEqual({ kind: "needs_signature" });
  });
});

describe("fixture masternodes: operations", () => {
  it("mn-ops: every state and kind, newest first, a dissolve's debit is its penalty and fee", async () => {
    install("mn-ops");
    const ops = await cmd.mnOperations(W);
    expect(ops.map((o) => [o.id, o.kind, o.state.kind, o.overdue])).toEqual([
      [OP_NEEDS_SIGNATURE, "registration", "needs_signature", false],
      [OP_INPUTS_SPENT, "registration", "inputs_spent", false],
      [OP_SIGNED, "registration", "signed", false],
      [OP_NOT_SENT, "registration", "not_sent", false],
      [OP_SENT, "registration", "sent", false],
      [OP_SENT_OVERDUE, "registration", "sent", true],
      [OP_DISSOLVE, "dissolve", "sent", true],
      [OP_DISSOLVE_FOREIGN, "dissolve", "sent", false],
      [OP_UPDATE, "update_registrar", "sent", false],
      [OP_CONFLICTED, "registration", "conflicted", false],
      [OP_CONFIRMED, "registration", "confirmed", false],
    ]);
    const by = new Map(ops.map((o) => [o.id, o]));
    expect(by.get(OP_DISSOLVE)!.debit).toBe(DISSOLVE_PENALTY + by.get(OP_DISSOLVE)!.fee);
    expect(by.get(OP_DISSOLVE_FOREIGN)!.debit).toBe(0);
    expect(by.get(OP_NOT_SENT)!.lastError).toBe("send.no_peers");
    expect(by.get(OP_NEEDS_SIGNATURE)!.signRequest).not.toBeNull();
    expect(by.get(OP_UPDATE)!.proTxHash).toBe(HOME);
    expect(new Set(ops.map((o) => o.createdAt)).size).toBe(ops.length);
  });

  it("resends signed, not_sent and sent rows, charged the row's debit", async () => {
    install("mn-ops");
    const seen = await events();
    for (const id of [OP_NEEDS_SIGNATURE, OP_INPUTS_SPENT, OP_CONFLICTED, OP_CONFIRMED]) {
      expect(await code(cmd.mnResendOperation(W, id, await mnGrant(10_000)))).toBe("masternodes.operation_state");
    }
    await expect(cmd.mnResendOperation(W, OP_DISSOLVE, await mnGrant(REG_FEE.regular + 10_000))).rejects.toMatchObject({ code: "send.grant_exceeded", data: { outflow: DISSOLVE_PENALTY + REG_FEE.regular } });
    expect(seen).toEqual([]);
    for (const id of [OP_SIGNED, OP_NOT_SENT, OP_SENT_OVERDUE, OP_UPDATE]) {
      expect(await cmd.mnResendOperation(W, id, await mnGrant(REG_FEE.regular))).toMatchObject({ id, state: { kind: "sent" }, overdue: false, lastError: null });
    }
    expect((await cmd.mnResendOperation(W, OP_DISSOLVE, await mnGrant(DISSOLVE_PENALTY + REG_FEE.regular))).state).toEqual({ kind: "sent" });
    expect((await cmd.mnResendOperation(W, OP_DISSOLVE_FOREIGN, await mnGrant(0))).state).toEqual({ kind: "sent" });
    expect(seen).toHaveLength(6);
  });

  it("discards only what can't be out, unlocking its keys", async () => {
    install("mn-ops");
    const seen = await events();
    for (const id of [OP_SENT, OP_SENT_OVERDUE, OP_DISSOLVE, OP_UPDATE, OP_CONFLICTED, OP_CONFIRMED]) {
      await expect(cmd.mnDiscardOperation(W, id)).rejects.toMatchObject({ code: "masternodes.operation_state", data: { kind: "operationState" } });
    }
    await expect(cmd.mnDiscardOperation(W, "XY".repeat(16))).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(cmd.mnDiscardOperation(W, "ab".repeat(16))).rejects.toMatchObject({ code: "masternodes.operation_not_found" });
    expect(seen).toEqual([]);
    for (const id of [OP_NEEDS_SIGNATURE, OP_INPUTS_SPENT, OP_SIGNED, OP_NOT_SENT]) expect(await cmd.mnDiscardOperation(W, id)).toBeNull();
    expect(seen).toHaveLength(4);
    expect((await cmd.mnOperations(W)).map((o) => o.id)).toEqual([OP_SENT, OP_SENT_OVERDUE, OP_DISSOLVE, OP_DISSOLVE_FOREIGN, OP_UPDATE, OP_CONFLICTED, OP_CONFIRMED]);
  });

  it("releases and locks a collateral of the wallet's masternodes, round trip", async () => {
    install("mn-list");
    const seen = await events();
    const released = async () => (await cmd.mnList(W)).masternodes.find((r) => r.proTxHash === HOME)!.collateralReleased;
    expect(await released()).toBe(false);
    await expect(cmd.mnReleaseCollateral(W, "xyz")).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(cmd.mnReleaseCollateral(W, CLUB)).rejects.toMatchObject({ code: "masternodes.not_found" });
    await expect(cmd.mnReleaseCollateral(W, FRIEND)).rejects.toMatchObject({ code: "masternodes.not_found" });
    await expect(cmd.mnReleaseCollateral("ff".repeat(32), HOME)).rejects.toMatchObject({ code: "wallet_not_found" });
    expect(seen).toEqual([]);
    expect(await cmd.mnReleaseCollateral(W, HOME.toUpperCase())).toBeNull();
    expect(await released()).toBe(true);
    expect(await cmd.mnLockCollateral(W, HOME)).toBeNull();
    expect(await released()).toBe(false);
    expect(seen).toHaveLength(2);
  });

  it("mn-released: Home's collateral reads released", async () => {
    install("mn-released");
    expect((await cmd.mnList(W)).masternodes.filter((r) => r.collateralReleased).map((r) => r.proTxHash)).toEqual([HOME]);
  });
});

describe("Change keys or payout (MG-10 S3, ProUpRegTx)", () => {
  const keep = { kind: "keep" } as const;
  const req = (proTxHash: string, patch: Partial<UpdateRegistrarRequest> = {}): UpdateRegistrarRequest => ({ proTxHash, operator: keep, voting: keep, payout: { kind: "new_address" } as const, ...patch });

  it("plans what changes, signed by the keychain or the attached owner key", async () => {
    install("mn-list");
    const p = await cmd.mnPreviewUpdateRegistrar(W, req(HOME, { operator: { kind: "next_unused" } }));
    expectShape("UpdateRegistrarPreview", p);
    expectShape("ProTxTerms", p.terms);
    expect(p).toMatchObject({ operatorChanges: true, poseBans: true, votingChanges: false, payoutChanges: true, ownerKey: { kind: "keychain", index: 0 } });
    expect(p.totalDebit).toBe(p.fee);
    // Office's list key is the legacy form of keychain key 1: keeping it is no change (MG-04 D39).
    const office = await cmd.mnPreviewUpdateRegistrar(W, req(OFFICE, { operator: { kind: "keychain", index: 1 } }));
    expect(office).toMatchObject({ operatorChanges: false, poseBans: false });
  });

  it("refuses without the owner key, the registration or the list", async () => {
    const f = install("mn-list");
    await expect(cmd.mnPreviewUpdateRegistrar(W, req(CLUB))).rejects.toMatchObject({ code: "masternodes.no_owner_key" });
    await expect(cmd.mnPreviewUpdateRegistrar(W, req(HOME, { operator: { kind: "given", publicKey: "ab" } }))).rejects.toMatchObject({ code: "invalid_argument" });
    f.world.masternodes.listState = { synced: false, height: null };
    await expect(cmd.mnPreviewUpdateRegistrar(W, req(HOME))).rejects.toMatchObject({ code: "masternodes.list_unavailable" });
  });

  it("sends under a masternodeOp grant covering the fee, then blocks the next until listed (MG-04 D49)", async () => {
    install("mn-list");
    const seen = await events();
    const p = await cmd.mnPreviewUpdateRegistrar(W, req(HOME));
    const small = (await cmd.authorize({ kind: "masternodeOp", maxDuffs: p.totalDebit - 1 }, W, PASS)).id;
    await expect(cmd.mnUpdateRegistrar(W, req(HOME), small)).rejects.toMatchObject({ code: "send.grant_exceeded", data: { kind: "spendCap", outflow: p.totalDebit } });
    const vote = (await cmd.authorize({ kind: "governance" }, W, PASS)).id;
    await expect(cmd.mnUpdateRegistrar(W, req(HOME), vote)).rejects.toMatchObject({ code: "platform.grant_invalid" });
    const grant = (await cmd.authorize({ kind: "masternodeOp", maxDuffs: p.totalDebit }, W, PASS)).id;
    const op = await cmd.mnUpdateRegistrar(W, req(HOME), grant);
    expectShape("ProTxOperation", op);
    expect(op).toMatchObject({ kind: "update_registrar", proTxHash: HOME, state: { kind: "sent" }, fee: p.fee });
    expect(seen.some((e) => e.kind === "masternodes")).toBe(true);
    await expect(cmd.mnUpdateRegistrar(W, req(HOME), grant)).rejects.toMatchObject({ code: "masternodes.update_pending" });
    await expect(cmd.mnPreviewUpdateRegistrar(W, req(HOME))).rejects.toMatchObject({ code: "masternodes.update_pending", data: { kind: "operation", operation: op.id } });
  });

  it("seeds Home's change saved or sent; either refuses a new one, naming it (#438)", async () => {
    for (const [scenario, state] of [["mn-update-saved", "not_sent"], ["mn-update-sent", "sent"]] as const) {
      install(scenario);
      const [op] = await cmd.mnOperations(W);
      expect(op).toMatchObject({ id: OP_UPDATE, kind: "update_registrar", proTxHash: HOME, state: { kind: state } });
      await expect(cmd.mnPreviewUpdateRegistrar(W, req(HOME))).rejects.toMatchObject({ code: "masternodes.update_pending", data: { kind: "operation", operation: OP_UPDATE } });
      clearMocks();
    }
  });

  it("previews a change behind an abandoned one, and refuses sending it, naming it, before the grant is used (update.rs resolve_update)", async () => {
    const f = install("mn-update-abandoned");
    await cmd.mnPreviewUpdateRegistrar(W, req(HOME));
    const grant = (await cmd.authorize({ kind: "masternodeOp", maxDuffs: 100_000_000 }, W, PASS)).id;
    await expect(cmd.mnUpdateRegistrar(W, req(HOME), grant)).rejects.toMatchObject({ code: "masternodes.update_abandoned_pending", data: { kind: "operation", operation: OP_UPDATE } });
    expect(f.world.grants.find((g) => g.grant.id === grant)!.used).toBe(false);
    expect((await cmd.mnOperations(W)).map((o) => o.id)).toEqual([OP_UPDATE]);
  });

  it("refuses Send again of a change saved behind an abandoned one, naming it, and keeps the grant (resend_conflict)", async () => {
    const f = install("mn-update-saved-behind-abandoned");
    const grant = (await cmd.authorize({ kind: "masternodeOp", maxDuffs: 100_000_000 }, W, PASS)).id;
    await expect(cmd.mnResendOperation(W, OP_UPDATE_SAVED, grant)).rejects.toMatchObject({ code: "masternodes.update_abandoned_pending", data: { kind: "operation", operation: OP_UPDATE } });
    expect(f.world.grants.find((g) => g.grant.id === grant)!.used).toBe(false);
    expect(f.world.masternodes.operations.find((o) => o.id === OP_UPDATE_SAVED)!.state).toEqual({ kind: "not_sent" });
  });

  it("refuses Send again of a change saved behind a sent one, naming it, and keeps the grant (resend_conflict)", async () => {
    const f = install("mn-update-saved-behind-sent");
    const grant = (await cmd.authorize({ kind: "masternodeOp", maxDuffs: 100_000_000 }, W, PASS)).id;
    await expect(cmd.mnResendOperation(W, OP_UPDATE_SAVED, grant)).rejects.toMatchObject({ code: "masternodes.update_pending", data: { kind: "operation", operation: OP_UPDATE } });
    expect(f.world.grants.find((g) => g.grant.id === grant)!.used).toBe(false);
    expect(f.world.masternodes.operations.find((o) => o.id === OP_UPDATE_SAVED)!.state).toEqual({ kind: "not_sent" });
    const again = (await cmd.authorize({ kind: "masternodeOp", maxDuffs: 100_000_000 }, W, PASS)).id;
    await expect(cmd.mnResendOperation(W, OP_UPDATE, again)).rejects.toMatchObject({ code: "masternodes.update_superseded", data: { kind: "operation", operation: OP_UPDATE_SAVED } });
  });

  it("refuses a payout to the owner or voting key's address (bad-protx-payee-reuse)", async () => {
    install("mn-list");
    await expect(cmd.mnPreviewUpdateRegistrar(W, req(HOME, { payout: { kind: "address", address: keyInfo("voting", 0).address! } }))).rejects.toMatchObject({
      code: "masternodes.protx_refused",
      data: { kind: "reason", reason: "bad-protx-payee-reuse" },
    });
  });

  it("saves an update that found no peers as not_sent; resend or discard it (MG-04 D7)", async () => {
    const f = install("mn-update-not-sent");
    const p = await cmd.mnPreviewUpdateRegistrar(W, req(HOME));
    const g = async () => (await cmd.authorize({ kind: "masternodeOp", maxDuffs: p.totalDebit }, W, PASS)).id;
    const op = await cmd.mnUpdateRegistrar(W, req(HOME), await g());
    expect(op).toMatchObject({ state: { kind: "not_sent" }, lastError: "send.no_peers" });
    await expect(cmd.mnPreviewUpdateRegistrar(W, req(HOME))).rejects.toMatchObject({ code: "masternodes.update_pending" });
    expect((await cmd.mnOperations(W)).map((o) => o.id)).toEqual([op.id]);
    const sent = await cmd.mnResendOperation(W, op.id, await g());
    expect(sent.state).toEqual({ kind: "sent" });
    await expect(cmd.mnDiscardOperation(W, op.id)).rejects.toMatchObject({ code: "masternodes.operation_state" });
    f.world.masternodes.nextUpdateNotSent = true;
    f.world.masternodes.operations = [];
    const again = await cmd.mnUpdateRegistrar(W, req(HOME), await g());
    expect(await cmd.mnDiscardOperation(W, again.id)).toBeNull();
    expect(await cmd.mnOperations(W)).toEqual([]);
    await expect(cmd.mnPreviewUpdateRegistrar(W, req(HOME))).resolves.toMatchObject({ fee: p.fee });
  });

  it("refuses an operator key another listed masternode uses (bad-protx-dup-key)", async () => {
    install("mn-list");
    await expect(cmd.mnPreviewUpdateRegistrar(W, req(OFFICE, { operator: { kind: "keychain", index: 0 } }))).rejects.toMatchObject({
      data: { kind: "reason", reason: "bad-protx-dup-key" },
    });
  });
});
