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
import { CLUB, FRIEND, HOME, KEY_OTHER, KEY_OWNER, KEY_PAYOUT, KEY_VOTING, MAX_LABEL, OFFICE, OLD, SPARE, TEST_VECTOR_WIF, keyInfo } from "./masternodes";
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
  "mn-keys",
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
    if (scenario !== "mn-syncing") for (const l of await cmd.mnFind("198.51.100.21")) expectShape("ListedMasternode", l);
  });

  it("mn-list: Home, Office banned, Club tracked with a key, Old retired", async () => {
    install("mn-list");
    const o = await cmd.mnList(W);
    const by = new Map(o.masternodes.map((r) => [r.proTxHash, r]));
    expect([...by.keys()]).toEqual([HOME, OFFICE, CLUB, OLD]);
    expect(by.get(OFFICE)).toMatchObject({ status: "pose_banned", mine: true, roles: { owner: true, collateral: true } });
    expect(by.get(OFFICE)!.bannedSeenAt).not.toBeNull();
    expect(by.get(CLUB)).toMatchObject({ kind: "evonode", mine: false, tracked: true, collateral: { kind: "unresolved" }, roles: { attached: [{ role: "owner", check: "unchecked" }] } });
    expect(by.get(OLD)!.status).toBe("removed");
    expect(o.unattributed30d).toBe(0);
    expect(o.payouts30d).toBe(by.get(HOME)!.payouts30d);
    expect(o.payouts30d).toBe(4 * 144_270_000);
    expect((await cmd.mnMasternode(W, HOME)).registrar).toEqual({ kind: "available", signer: "keychain" });
    expect((await cmd.mnMasternode(W, CLUB)).registrar).toEqual({ kind: "registration_unavailable" });
    expect((await cmd.mnMasternode(W, OLD)).list).toBeNull();
    expect((await cmd.mnPayouts(W, HOME, null)).length).toBe(5);
    const since = (await cmd.mnPayouts(W, HOME, null))[2].timestamp!;
    expect((await cmd.mnPayouts(W, HOME, since)).length).toBe(3);
  });

  it("mn-v24-payouts: flagged records, a shared payout address and unattributed income", async () => {
    install("mn-v24-payouts");
    const o = await cmd.mnList(W);
    const home = o.masternodes.find((r) => r.proTxHash === HOME)!;
    const office = o.masternodes.find((r) => r.proTxHash === OFFICE)!;
    expect(o.unattributed30d).toBeGreaterThan(0);
    expect(home.payoutsIncomplete && office.payoutsIncomplete).toBe(true);
    expect(home.payoutAddress).toBe(office.payoutAddress);
    expect((await cmd.mnPayouts(W, HOME, null)).some((p) => p.unattributed)).toBe(true);
    const fixtureNow = Math.floor(new Date(2026, 9, 6, 12).getTime() / 1000);
    expect(office.lastPayout).toMatchObject({ unattributed: true });
    expect(office.lastPayout!.timestamp!).toBeLessThan(fixtureNow - 30 * DAY);
  });

  it("mn-empty and mn-empty-coin: no rows, with and without a collateral-shaped coin", async () => {
    install("mn-empty");
    expect(await cmd.mnList(W)).toMatchObject({ masternodes: [], collateralCoins: [] });
    install("mn-empty-coin");
    const o = await cmd.mnList(W);
    expect(o.masternodes).toEqual([]);
    expect(o.collateralCoins).toMatchObject([{ amount: 1_000 * 100_000_000, userLocked: false }]);
  });

  it("mn-syncing: own rows read Unknown and list reads refuse list_unavailable", async () => {
    install("mn-syncing");
    expect(await cmd.mnListState()).toEqual({ synced: false, height: null });
    const o = await cmd.mnList(W);
    expect(o.masternodes.filter((r) => r.mine).every((r) => r.status === "unknown")).toBe(true);
    await expect(cmd.mnFind(HOME)).rejects.toMatchObject({ code: "masternodes.list_unavailable" });
    await expect(cmd.mnMasternode(W, FRIEND)).rejects.toMatchObject({ code: "masternodes.list_unavailable" });
    await expect(cmd.mnTrack(FRIEND, null)).rejects.toMatchObject({ code: "masternodes.list_unavailable" });
    await expect(cmd.mnAttachKey(CLUB, "voting", KEY_VOTING)).rejects.toMatchObject({ code: "masternodes.list_unavailable" });
  });

  it("mn-watchonly: the keychain refuses watch_only", async () => {
    install("mn-watchonly");
    await expect(cmd.mnKeys(W, "owner", 0, 20)).rejects.toMatchObject({ code: "masternode.watch_only" });
    await expect(cmd.mnRevealKey(W, "owner", 0, "g")).rejects.toMatchObject({ code: "masternode.watch_only" });
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
    expect(row).toMatchObject({ label: "Friend's evonode", kind: "evonode", tracked: true, mine: false, typeIndex: 2 });
    expect((await cmd.mnMasternode(W, FRIEND)).trackedAt).not.toBeNull();
  });

  it("checks each attached key by role and never echoes it", async () => {
    install("mn-track");
    await cmd.mnTrack(SPARE, null);
    await cmd.mnTrack(FRIEND, null);
    expect(await cmd.mnAttachKey(CLUB, "payout", KEY_PAYOUT)).toBe("matched");
    expect(await cmd.mnAttachKey(CLUB, "payout", KEY_OTHER)).toBe("unchecked");
    expect(await cmd.mnAttachKey(CLUB, "voting", KEY_VOTING)).toBe("matched");
    expect(await cmd.mnAttachKey(FRIEND, "owner", KEY_OWNER)).toBe("unchecked");
    expect(await cmd.mnAttachKey(SPARE, "owner", TEST_VECTOR_WIF)).toBe("unchecked");
    for (const [hash, role, key, code] of [
      ["4e", "owner", KEY_OWNER, "invalid_argument"],
      [CLUB, "owner", "not a key", "invalid_argument"],
      [CLUB, "owner", KEY_OWNER.toUpperCase().slice(2), "invalid_argument"],
      [HOME, "owner", TEST_VECTOR_WIF, "masternodes.not_tracked"],
      [CLUB, "voting", TEST_VECTOR_WIF, "masternodes.key_mismatch"],
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
  });
});
