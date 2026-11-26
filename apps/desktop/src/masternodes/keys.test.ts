// Masternode keys (MG-10 §1.5, D8): matching a keychain key to known masternodes, and the reveal
// behind the RevealSecret gate, which follows Show Recovery Phrase's lock policy.

import { describe, expect, it } from "vitest";
import type { MasternodeDetail } from "../api/generated/MasternodeDetail";
import type { MasternodeKeyInfo } from "../api/generated/MasternodeKeyInfo";
import type { MasternodeKeyRoleName } from "../api/generated/MasternodeKeyRoleName";
import type { RevealedMasternodeKey } from "../api/generated/RevealedMasternodeKey";
import { appError, FakeWorld, status, WALLET_A } from "../onboarding/testWorld";
import { canCopySecret, KeyRevealModel, keyMatches, keysErrorText, knownMatches, type ReadState } from "./keys";

const key = (role: MasternodeKeyRoleName, over: Partial<MasternodeKeyInfo> = {}): MasternodeKeyInfo => ({
  role,
  index: 0,
  derivationPath: "m/9'/1'/3'/1'/0'",
  address: null,
  publicKeyHex: "aa".repeat(48),
  legacyPublicKeyHex: null,
  platformNodeId: null,
  ...over,
});

const detail = (hash: string, over: { owner?: string; voting?: string; operator?: string; node?: string; service?: string }): MasternodeDetail =>
  ({
    row: { proTxHash: hash, service: over.service ?? null },
    list: over.operator || over.voting || over.node ? { operatorPublicKey: over.operator ?? "", votingAddress: over.voting ?? "", platformNodeId: over.node ?? null, service: over.service ?? null } : null,
    registration: over.owner ? { ownerAddress: over.owner, votingAddress: null } : null,
  }) as unknown as MasternodeDetail;

const name = (d: MasternodeDetail) => `node ${d.row.proTxHash}`;

describe("matching keys to known masternodes (D8)", () => {
  const nodes = [
    detail("h1", { owner: "yOwner1", voting: "yVote1", operator: "bb".repeat(48), service: "203.0.113.7:19999" }),
    detail("h2", { voting: "yVote2", operator: "cc".repeat(48), node: "ab12" }),
  ];

  it("matches owner and voting keys by address, exactly", () => {
    expect(keyMatches(key("owner", { address: "yOwner1" }), nodes, name)).toEqual([{ proTxHash: "h1", name: "node h1", service: "203.0.113.7:19999" }]);
    expect(keyMatches(key("voting", { address: "yVote2" }), nodes, name).map((m) => m.proTxHash)).toEqual(["h2"]);
    expect(keyMatches(key("owner", { address: "yVote1" }), nodes, name)).toEqual([]);
    // Base58Check is case-sensitive: another case is another address.
    expect(keyMatches(key("owner", { address: "yowner1" }), nodes, name)).toEqual([]);
  });

  it("takes the live voting address over the registration's after a key change", () => {
    const changed = [{ ...detail("h3", { voting: "yNewVote" }), registration: { ownerAddress: null, votingAddress: "yOldVote" } } as unknown as MasternodeDetail];
    expect(keyMatches(key("voting", { address: "yNewVote" }), changed, name).map((m) => m.proTxHash)).toEqual(["h3"]);
    expect(keyMatches(key("voting", { address: "yOldVote" }), changed, name)).toEqual([]);
  });

  it("matches an operator key by either BLS form", () => {
    expect(keyMatches(key("operator", { publicKeyHex: "bb".repeat(48) }), nodes, name).map((m) => m.proTxHash)).toEqual(["h1"]);
    expect(keyMatches(key("operator", { legacyPublicKeyHex: "cc".repeat(48) }), nodes, name).map((m) => m.proTxHash)).toEqual(["h2"]);
    expect(keyMatches(key("operator"), nodes, name)).toEqual([]);
  });

  it("matches a platform node key by node id", () => {
    expect(keyMatches(key("platform_node", { platformNodeId: "AB12" }), nodes, name).map((m) => m.proTxHash)).toEqual(["h2"]);
    expect(keyMatches(key("platform_node", { platformNodeId: "ffff" }), nodes, name)).toEqual([]);
  });
});

describe("when key matches are settled (D8)", () => {
  const read = <T>(data: T | undefined, over: Partial<ReadState<T>> = {}): ReadState<T> => ({ data, isError: false, isPending: data === undefined, isFetching: false, isPaused: false, ...over });
  const office = detail("11", { owner: "yOffice" });

  it("matches only once the list and every detail have settled with success", () => {
    expect(knownMatches(read(["11"]), [read(office)])).toEqual({ details: [office], state: "done" });
    expect(knownMatches(read(undefined), [])).toEqual({ details: [], state: "checking" });
    expect(knownMatches(read(["11"]), [read<MasternodeDetail>(undefined)])).toEqual({ details: [], state: "checking" });
  });

  it("is checking, with no match from cached data, while a refresh runs", () => {
    expect(knownMatches(read(["11"], { isFetching: true }), [read(office)])).toEqual({ details: [], state: "checking" });
    expect(knownMatches(read(["11"]), [read(office, { isFetching: true })])).toEqual({ details: [], state: "checking" });
  });

  it("is checking, with no match from cached data, while a refresh waits offline", () => {
    expect(knownMatches(read(["11"]), [read(office, { isPaused: true })])).toEqual({ details: [], state: "checking" });
    expect(knownMatches(read(["11"], { isPaused: true }), [read(office)])).toEqual({ details: [], state: "checking" });
  });

  it("is failed, with no match from cached data, after a refresh fails", () => {
    expect(knownMatches(read(["11"]), [read(office, { isError: true })])).toEqual({ details: [], state: "failed" });
    expect(knownMatches(read(["11"], { isError: true }), [read(office)])).toEqual({ details: [], state: "failed" });
    // Try again: checking until the retry settles.
    expect(knownMatches(read(["11"], { isError: true, isFetching: true }), [read(office)])).toEqual({ details: [], state: "checking" });
  });
});

const SECRET: RevealedMasternodeKey = { privateKeyHex: "11".repeat(32), wif: "cVt4o7BGAig1UXywgGSmARhxMdzP5qvQsxKkSsc1XEkw3tDTQFpy", tenderdashKey: null };

class KeysWorld extends FakeWorld {
  reveals: { role: string; index: number; grant: string }[] = [];
  revealFails = false;
  mnRevealKey = async (_wallet: string, role: MasternodeKeyRoleName, index: number, grant: string) => {
    this.reveals.push({ role, index, grant });
    if (this.revealFails) throw appError("masternode.grant_invalid");
    return SECRET;
  };
}

function world(encrypted = true) {
  const w = new KeysWorld();
  w.status = status({ state: encrypted ? "unlocked" : "unencrypted", encrypted, walletsWithSecrets: [WALLET_A] });
  return w;
}

describe("revealing a masternode key", () => {
  it("asks for the passphrase on an encrypted vault and reveals with a revealSecret grant", async () => {
    const w = world();
    const m = new KeyRevealModel(w, WALLET_A, "operator", 3);
    await m.load();
    expect(m.step).toBe("hidden");
    await m.reveal("");
    expect(m.secret).toBeNull();
    expect(w.authorizations).toEqual([]);
    await m.reveal("wrong");
    expect(m.secret).toBeNull();
    await m.reveal("right");
    expect(m.step).toBe("shown");
    expect(m.secret).toEqual(SECRET);
    // The wrong passphrase was tried too, and counted by the vault, as the lock screen's attempts are.
    expect(w.authorizations).toEqual(Array(2).fill({ kind: "revealSecret", walletId: WALLET_A, credential: "passphrase" }));
    expect(w.reveals).toMatchObject([{ role: "operator", index: 3 }]);
  });

  it("needs no passphrase on an unencrypted vault", async () => {
    const w = world(false);
    const m = new KeyRevealModel(w, WALLET_A, "owner", 0);
    await m.load();
    await m.reveal("");
    expect(m.secret).toEqual(SECRET);
    expect(w.authorizations).toEqual([{ kind: "revealSecret", walletId: WALLET_A, credential: "unencrypted" }]);
  });

  it("drops the secret on hide and on dispose", async () => {
    const w = world(false);
    const m = new KeyRevealModel(w, WALLET_A, "owner", 0);
    await m.load();
    await m.reveal("");
    m.hide();
    expect(m.secret).toBeNull();
    expect(m.step).toBe("hidden");
    await m.reveal("");
    m.dispose();
    expect(m.secret).toBeNull();
  });

  it("keeps nothing from a reveal that finishes after the panel closed, and revokes a grant the reveal did not use", async () => {
    const w = world(false);
    const m = new KeyRevealModel(w, WALLET_A, "voting", 1);
    await m.load();
    const pending = m.reveal("");
    m.dispose();
    await pending;
    expect(m.secret).toBeNull();
    w.revealFails = true;
    const m2 = new KeyRevealModel(w, WALLET_A, "voting", 1);
    await m2.load();
    await m2.reveal("");
    expect(m2.secret).toBeNull();
    expect(m2.step).toBe("hidden");
    expect(m2.error).toBe("The authorization expired. Try again.");
    expect(w.revoked).toEqual(["grant-1", "grant-2"]);
    expect(keysErrorText(appError("masternode.vault_locked"))).toBe("Unlock the wallet and try again.");
  });
});

describe("revealing a masternode key when something goes wrong", () => {
  it("keeps Reveal off when the vault's state could not be read", async () => {
    const w = world();
    w.lockStatus = async () => {
      throw appError("storage");
    };
    const m = new KeyRevealModel(w, WALLET_A, "owner", 0);
    await m.load();
    expect(m.step).toBe("failed");
    await m.reveal("right");
    expect(w.authorizations).toEqual([]);
  });

  it("keeps Reveal off when only the lock policy's own read fails", async () => {
    const w = world();
    const first = w.lockStatus.bind(w);
    let reads = 0;
    w.lockStatus = async () => {
      if (++reads > 1) throw appError("storage");
      return first();
    };
    const m = new KeyRevealModel(w, WALLET_A, "owner", 0);
    await m.load();
    expect(reads).toBe(2);
    expect(m.step).toBe("failed");
    await m.reveal("right");
    expect(w.authorizations).toEqual([]);
  });

  it("stays failed when the window loses focus", async () => {
    const w = world();
    w.lockStatus = async () => {
      throw appError("storage");
    };
    const m = new KeyRevealModel(w, WALLET_A, "owner", 0);
    await m.load();
    m.hide();
    expect(m.step).toBe("failed");
    await m.reveal("right");
    expect(w.authorizations).toEqual([]);
  });

  it("revokes a newer reveal's grant on close after Hide dropped an older one", async () => {
    const w = world(false);
    const releases: (() => void)[] = [];
    w.mnRevealKey = (_wallet, role, index, grant) =>
      new Promise((resolve) => {
        w.reveals.push({ role, index, grant });
        releases.push(() => resolve(SECRET));
      });
    const m = new KeyRevealModel(w, WALLET_A, "owner", 0);
    await m.load();
    const first = m.reveal("");
    await expect.poll(() => w.reveals.length).toBe(1);
    m.hide();
    expect(w.revoked).toEqual([w.reveals[0].grant]);
    const second = m.reveal("");
    await expect.poll(() => w.reveals.length).toBe(2);
    releases[0]();
    await first;
    expect(m.secret).toBeNull();
    expect(m.step).toBe("working");
    m.dispose();
    expect(w.revoked).toEqual([w.reveals[0].grant, w.reveals[1].grant]);
    releases[1]();
    await second;
    expect(m.secret).toBeNull();
  });

  it("leaves a newer reveal running when an older one's authorization fails after Hide", async () => {
    const w = world(false);
    const answers: ((ok: boolean) => void)[] = [];
    const authorize = w.authorize.bind(w);
    w.authorize = (...args: Parameters<typeof authorize>) =>
      new Promise((resolve, reject) => answers.push((ok) => (ok ? resolve(authorize(...args)) : reject(appError("vault.grant_invalid")))));
    const m = new KeyRevealModel(w, WALLET_A, "owner", 0);
    await m.load();
    const first = m.reveal("");
    m.hide();
    const second = m.reveal("");
    expect(m.step).toBe("working");
    answers[0](false);
    await first;
    expect(m.step).toBe("working");
    expect(m.error).toBeNull();
    expect(w.revoked).toEqual([]);
    answers[1](true);
    await second;
    expect(m.step).toBe("shown");
  });

  it("revokes the grant of a reveal still running when the panel closes", async () => {
    const w = world(false);
    let release: () => void = () => undefined;
    w.mnRevealKey = (_wallet, role, index, grant) =>
      new Promise((resolve) => {
        w.reveals.push({ role, index, grant });
        release = () => resolve(SECRET);
      });
    const m = new KeyRevealModel(w, WALLET_A, "owner", 0);
    await m.load();
    const pending = m.reveal("");
    await expect.poll(() => w.reveals.length).toBe(1);
    m.dispose();
    expect(w.revoked).toEqual([w.reveals[0].grant]);
    release();
    await pending;
    expect(m.secret).toBeNull();
  });
});

describe("which revealed secrets copy (#428, MG ruling (c))", () => {
  it("copies operator and platform node secrets only", () => {
    expect(["owner", "voting", "operator", "platform_node"].filter((r) => canCopySecret(r as MasternodeKeyRoleName))).toEqual(["operator", "platform_node"]);
  });
});
