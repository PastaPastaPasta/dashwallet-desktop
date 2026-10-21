// Settings ▸ Advanced ▸ Identities (DP6-01), by parity ID: IOS-071 (the identities list with the
// main one marked, Set as main, each identity's keys, Refresh, Find my identities, the look-up).

import { describe, expect, it, vi } from "vitest";
import type { FoundIdentity } from "../api/generated/FoundIdentity";
import type { IdentityKeyInfo } from "../api/generated/IdentityKeyInfo";
import type { IdentitySummary } from "../api/generated/IdentitySummary";
import { appError, WALLET_A } from "../onboarding/testWorld";
import { fakeVault } from "../tools/testApi";
import { discoverText, findInput, foundModel, identityErrorText, identityRow, IdentitiesModel, keyRow, keyRows, listModel, refreshAllText } from "./identities";

const ID_A = "4EfA9Jrvv3nnCFdSf7fad59851iiTRZ6Wcu6YVJ4iSeF";
const ID_B = "8GopLQQPjKr2gXjNEt9G8k2PzRZrdbBqQBNTCXV2DYiN";

function summary(patch: Partial<IdentitySummary> = {}): IdentitySummary {
  return {
    identity: ID_A,
    index: 0,
    names: ["alice"],
    mainName: "alice",
    namesUpdating: false,
    namesCheck: null,
    isMain: false,
    balance: "1000",
    balanceAsOf: null,
    balanceStale: false,
    hasDashpayKeys: true,
    profile: null,
    unverified: false,
    creditLevel: "ok",
    ...patch,
  };
}

function key(patch: Partial<IdentityKeyInfo> = {}): IdentityKeyInfo {
  return {
    id: 0,
    purpose: "authentication",
    securityLevel: "master",
    keyType: "ecdsaSecp256k1",
    publicKey: "02".padEnd(66, "a"),
    readOnly: false,
    disabledAt: null,
    contractBound: null,
    contractBoundDocumentType: null,
    ...patch,
  };
}

function setup() {
  const vault = fakeVault();
  const api = {
    ...vault.api,
    setMainIdentity: vi.fn(async () => null),
    setMainName: vi.fn(async () => null),
    identityRefresh: vi.fn(async () => ({ summary: summary(), revision: 2, publicKeys: [] })),
    identityRefreshAll: vi.fn(async () => [
      { identity: ID_A, failureCode: null },
      { identity: ID_B, failureCode: null },
    ]),
    identityFind: vi.fn(async (): Promise<FoundIdentity | null> => null),
    discoverIdentities: vi.fn(async () => 2),
  };
  return { model: new IdentitiesModel(api, WALLET_A), api, vault };
}

describe("the identities list (IOS-071 L1–L5)", () => {
  it("L1–L3: loading, failed, none", () => {
    expect(listModel({ isError: false })).toEqual({ kind: "loading" });
    expect(listModel({ isError: true })).toEqual({ kind: "failed" });
    expect(listModel({ data: [], isError: false })).toEqual({ kind: "none" });
  });

  it("L4: rows in index order; the main badge is on the main identity's row only", () => {
    const m = listModel({
      data: [summary({ identity: ID_B, index: 1, isMain: true, names: ["bob"], mainName: "bob" }), summary()],
      isError: false,
    });
    if (m.kind !== "rows") throw new Error(m.kind);
    expect(m.rows.map((r) => [r.identity, r.isMain, r.index, r.username])).toEqual([
      [ID_A, false, "Index 0", "@alice"],
      [ID_B, true, "Index 1", "@bob"],
    ]);
  });

  it("L4/L5: a row's names state, initials, balance and marks", () => {
    expect(identityRow(summary({ mainName: null, names: [] }))).toMatchObject({ username: "No username", noName: true });
    expect(identityRow(summary()).noName).toBe(false);
    expect(identityRow(summary({ mainName: null, namesUpdating: true })).username).toBe("Updating…");
    expect(identityRow(summary({ mainName: null, namesCheck: "failed" })).username).toBe("Username not loaded");
    expect(identityRow(summary({ profile: { displayName: "zoe", publicMessage: null, avatarUrl: null, avatarHash: null, avatarFingerprint: null, updatedAt: null } })).initials).toBe("Z");
    expect(identityRow(summary({ balance: null })).credits).toBeNull();
    const r = identityRow(summary({ balanceStale: true, unverified: true }));
    expect([r.stale, r.unverified]).toEqual([true, true]);
  });
});

describe("an identity's keys (IOS-071 detail)", () => {
  it("names each purpose, level and type; sorts by id", () => {
    const rows = keyRows([key({ id: 2, purpose: "encryption", securityLevel: "medium", keyType: "bls12381" }), key()]);
    expect(rows.map((r) => [r.id, r.purpose, r.level, r.type])).toEqual([
      [0, "Authentication", "Master", "ECDSA secp256k1"],
      [2, "Encryption", "Medium", "BLS12-381"],
    ]);
  });

  it("status: disabled over read-only over enabled", () => {
    expect(keyRow(key()).statusText).toBe("Enabled");
    expect(keyRow(key({ readOnly: true })).statusText).toBe("Read-only");
    expect(keyRow(key({ disabledAt: 1_700_000_000_000 }))).toMatchObject({ status: "disabled", statusText: "Disabled" });
    expect(keyRow(key({ readOnly: true, disabledAt: 1 })).status).toBe("disabled");
  });

  it("a contract bound: DashPay's contact requests by name, another contract by its short id", () => {
    const contract = "Bwr4WHCPz5rFVAD87RqTs3izo4zpzwsEdKPWUT1NS1C7";
    expect(keyRow(key({ contractBound: contract, contractBoundDocumentType: "contactRequest" })).bound).toBe("DashPay contact requests");
    expect(keyRow(key({ contractBound: contract })).bound).toBe("Bwr4WH…1NS1C7");
    expect(keyRow(key()).bound).toBeNull();
  });
});

describe("the look-up (IOS-071 find)", () => {
  it("takes an identity id or a username, with @ or .dash", () => {
    expect(findInput("  ")).toEqual({ kind: "empty" });
    expect(findInput(ID_A)).toEqual({ kind: "query", query: ID_A });
    expect(findInput(" @Alice.dash ")).toEqual({ kind: "query", query: "Alice" });
    expect(findInput("alice")).toEqual({ kind: "query", query: "alice" });
    for (const bad of ["ab", "-ab", "a.b.c", "al ice", "a_b", "x".repeat(64)]) expect(findInput(bad), bad).toEqual({ kind: "invalid" });
  });

  it("a found identity: how, names, revision, own and unverified", () => {
    const f: FoundIdentity = { identity: ID_B, foundBy: { kind: "name", label: "bob" }, names: [], balance: null, revision: null, publicKeys: [key()], own: true, unverified: true };
    expect(foundModel(f)).toMatchObject({ how: "Found by username @bob", usernames: "No username", revision: "Unknown", own: true, unverified: true });
    expect(foundModel({ ...f, foundBy: { kind: "id" }, names: ["bob", "bobby"], revision: 4 })).toMatchObject({ how: "Found by identity ID", usernames: "@bob, @bobby", revision: "4" });
  });

  it("searching → found / not found / failed / invalid; a newer look-up overtakes an older answer", async () => {
    const { model, api } = setup();
    await model.lookUp("ab");
    expect(model.find.kind).toBe("invalid");
    expect(api.identityFind).not.toHaveBeenCalled();
    await model.lookUp("nobody");
    expect(model.find.kind).toBe("notFound");
    api.identityFind.mockRejectedValueOnce(appError("platform.unavailable"));
    await model.lookUp("bob");
    expect(model.find).toEqual({ kind: "failed", text: "Dash Platform did not answer. Try again.", query: "bob" });
    api.identityFind.mockRejectedValueOnce(appError("invalid_argument"));
    await model.lookUp("bob");
    expect(model.find.kind).toBe("invalid");

    let answer: (f: FoundIdentity | null) => void = () => {};
    api.identityFind.mockImplementationOnce(() => new Promise((r) => (answer = r)));
    const slow = model.lookUp("old");
    api.identityFind.mockResolvedValueOnce({ identity: ID_B, foundBy: { kind: "id" }, names: [], balance: "5", revision: 1, publicKeys: [], own: false, unverified: false });
    await model.lookUp(ID_B);
    answer(null);
    await slow;
    expect(model.find.kind).toBe("found");
  });
});

describe("the page's actions (IOS-071)", () => {
  it("Set as main: one action at a time, its line on the row", async () => {
    const { model, api } = setup();
    let release: () => void = () => {};
    api.setMainIdentity.mockImplementationOnce(() => new Promise((r) => (release = () => r(null))));
    const first = model.setMain(ID_B);
    expect(model.running).toBe(`setMain:${ID_B}`);
    await model.refresh(ID_A);
    expect(api.identityRefresh).not.toHaveBeenCalled();
    release();
    await first;
    expect(api.setMainIdentity).toHaveBeenCalledWith(WALLET_A, ID_B);
    expect(model.result).toEqual({ identity: ID_B, ok: true, text: "Main identity changed" });
    expect(model.running).toBeNull();
  });

  it("a failed refresh shows its code's copy on the row", async () => {
    const { model, api } = setup();
    api.identityRefresh.mockRejectedValueOnce(appError("platform.trust_mismatch"));
    await model.refresh(ID_A);
    expect(model.result).toEqual({ identity: ID_A, ok: false, text: "This identity's data could not be verified against the network." });
    expect(identityErrorText(appError("internal"))).toBe("Something went wrong (internal).");
  });

  it("Refresh all (D9): the count, or how many failed and why", async () => {
    expect(refreshAllText([{ identity: ID_A, failureCode: null }])).toEqual({ ok: true, text: "Refreshed 1 identity" });
    expect(
      refreshAllText([
        { identity: ID_A, failureCode: "identity.not_found" },
        { identity: ID_B, failureCode: null },
      ]),
    ).toEqual({ ok: false, text: "1 identity could not be refreshed: This identity is not on Dash Platform." });
    const { model } = setup();
    await model.refreshAll();
    expect(model.result).toEqual({ identity: null, ok: true, text: "Refreshed 2 identities" });
  });

  it("Find my identities (D5): under an identityScan grant", async () => {
    const { model, api, vault } = setup();
    await model.discover();
    expect(vault.calls.authorize[0]).toMatchObject({ purpose: { kind: "identityScan" }, walletId: WALLET_A });
    expect(api.discoverIdentities).toHaveBeenCalledWith(WALLET_A, "grant-1");
    expect(model.result).toEqual({ identity: null, ok: true, text: "Found 2 new identities" });
    expect([discoverText(0), discoverText(1)]).toEqual(["No new identities found", "Found 1 new identity"]);
  });

  it("Set as main name: no line on success", async () => {
    const { model, api } = setup();
    await model.setMainName(ID_A, "alice2");
    expect(api.setMainName).toHaveBeenCalledWith(WALLET_A, ID_A, "alice2");
    expect(model.result).toBeNull();
  });
});
