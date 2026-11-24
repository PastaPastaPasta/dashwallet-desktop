// The registration wizard's model (registration.ts, MG-10 §1.6, D5).

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import type { Backend } from "../api/backend";
import type { AppError } from "../api/generated/commands";
import type { CollateralCoin } from "../api/generated/CollateralCoin";
import type { MnRegistrationPreview } from "../api/generated/MnRegistrationPreview";
import type { ProTxKeys } from "../api/generated/ProTxKeys";
import { t } from "../i18n";
import { collateralNote, HELD_CALLS, registrationSends, SENDING_CALLS, buildRequest, registrationCalls, coinsFor, finalAction, newDraft, normalOutpoint, refusalOf, registerCap, samePreview, serviceError, shareBasisPoints, stepErrors, type Ports, type RegDraft } from "./registration";

const R = "masternodes.register";
const COIN = 100_000_000;
const TX = "ab".repeat(32);
const ports: Ports = { own: 19999, other: { port: 9999, network: "Mainnet" }, network: "Testnet" };
const keys: ProTxKeys = {
  owner: { index: 3, address: "yOwnerAddress", publicKey: null, nodeId: null },
  voting: { index: 4, address: "yVotingAddress", publicKey: null, nodeId: null },
  operator: { index: 5, address: null, publicKey: "b".repeat(96), nodeId: null },
  platformNode: { index: 6, address: null, publicKey: null, nodeId: "d".repeat(40) },
};
const draft = (patch: Partial<RegDraft> = {}): RegDraft => ({ ...newDraft(), service: "203.0.113.7:19999", operatorKey: "8".repeat(96), ...patch });
const coin = (outpoint: string, patch: Partial<CollateralCoin> = {}): CollateralCoin => ({ outpoint, address: "yCoin", amount: 1000 * COIN, kind: "regular", confirmations: 10, userLocked: false, ...patch });
const preview = (patch: Partial<MnRegistrationPreview> = {}): MnRegistrationPreview => ({
  terms: {
    masternodeType: "regular",
    version: 2,
    service: "203.0.113.7:19999",
    ownerAddress: "yOwnerAddress",
    ownerIndex: 3,
    votingAddress: "yVotingAddress",
    votingIndex: 4,
    operatorPublicKey: "8".repeat(96),
    operatorIndex: null,
    operatorReward: 0,
    payoutAddress: "",
    collateral: "fund_new",
    collateralOutpoint: null,
    collateralAddress: null,
    platform: null,
    platformNodeIndex: null,
  },
  fee: 453,
  totalDebit: 453,
  collateralAmount: 1000 * COIN,
  ...patch,
});
const err = (code: string, data: AppError["data"] = null, message = code): AppError => ({ code: code as AppError["code"], message, data });

describe("buildRequest", () => {
  it("builds a hosted FundNew request with the keychain's owner and voting indices", () => {
    expect(buildRequest(draft({ operatorShare: "5" }), keys)).toEqual({
      kind: "regular",
      collateral: { kind: "fund_new" },
      service: "203.0.113.7:19999",
      operator: { kind: "given", publicKey: "8".repeat(96) },
      operatorReward: 500,
      ownerIndex: 3,
      voting: { kind: "keychain", index: 4 },
      payout: { kind: "new_address" },
      evonode: null,
    });
  });

  it("fills a self-run evonode's operator and node from the keychain, and the ports when typed", () => {
    const r = buildRequest(draft({ kind: "evonode", runner: "self", p2pPort: "36656", httpPort: "" }), keys);
    expect(r.operator).toEqual({ kind: "keychain", index: 5 });
    expect(r.evonode).toEqual({ node: { kind: "keychain", index: 6 }, p2pPort: 36656, httpPort: null });
  });

  it("takes an entered operator key and node id over the keychain's", () => {
    const r = buildRequest(draft({ kind: "evonode", runner: "self", operatorFromWallet: false, nodeFromWallet: false, nodeId: "e".repeat(40) }), keys);
    expect(r.operator).toEqual({ kind: "given", publicKey: "8".repeat(96) });
    expect(r.evonode?.node).toEqual({ kind: "given", nodeId: "e".repeat(40) });
  });

  it("normalizes dash-qt's txid:n to the engine's txid-n for an external collateral", () => {
    const r = buildRequest(draft({ collateral: "external", externalOutpoint: ` ${TX.toUpperCase()}:1 `, externalAddress: " yColl " }), keys);
    expect(r.collateral).toEqual({ kind: "external", outpoint: `${TX}-1`, address: "yColl" });
    expect(normalOutpoint(`${TX}-0`)).toBe(`${TX}-0`);
  });

  it("names other voting and payout addresses", () => {
    const r = buildRequest(draft({ voting: "address", votingAddress: "yVoter", payout: "address", payoutAddress: "yCold" }), keys);
    expect(r.voting).toEqual({ kind: "address", address: "yVoter" });
    expect(r.payout).toEqual({ kind: "address", address: "yCold" });
  });
});

describe("field checks", () => {
  it("accepts IPv4 with a port only (G3) and names another network's port", () => {
    expect(serviceError("203.0.113.7:19999", ports)).toBeNull();
    expect(serviceError("", ports)).toBe(t(`${R}.error.serviceMissing`));
    for (const bad of ["203.0.113.7", "[2001:db8::1]:19999", "abc.onion:19999", "300.0.0.1:19999", "203.0.113.7:0"]) expect(serviceError(bad, ports)).toBe(t(`${R}.error.serviceIpv4`));
    expect(serviceError("203.0.113.7:9999", ports)).toBe(t(`${R}.error.servicePort`, { port: "9999", otherNetwork: "Mainnet", network: "Testnet", own: "19999" }));
  });

  it("reads the operator's share as basis points, 0 to 100% with two decimals", () => {
    expect(shareBasisPoints("")).toBe(0);
    expect(shareBasisPoints("5")).toBe(500);
    expect(shareBasisPoints("2.25")).toBe(225);
    expect(shareBasisPoints("100")).toBe(10_000);
    for (const bad of ["100.01", "-1", "1.234", "five"]) expect(shareBasisPoints(bad)).toBeNull();
  });

  it("checks each step's fields before any call", () => {
    expect(stepErrors("collateral", draft({ collateral: "existing" }), ports, [], null)).toEqual({ outpoint: t(`${R}.error.pickCoin`) });
    expect(stepErrors("collateral", draft({ collateral: "external", externalOutpoint: "nope" }), ports, [], null)).toEqual({ externalOutpoint: t(`${R}.error.outpoint`), externalAddress: t(`${R}.error.address`) });
    expect(stepErrors("collateral", draft({ collateral: "external", externalOutpoint: `${TX}:1`, externalAddress: "y" }), ports, [coin(`${TX}-1`)], null)).toEqual({
      externalOutpoint: t(`${R}.error.collateralInWallet`, { amount: 1000 }),
    });
    expect(stepErrors("operator", draft({ operatorKey: "short", operatorShare: "101" }), ports, [], null)).toEqual({ operatorKey: t(`${R}.error.operatorKey`), operatorShare: t(`${R}.error.share`) });
    expect(stepErrors("operator", draft({ runner: "self", operatorKey: "" }), ports, [], null)).toEqual({});
    expect(stepErrors("operator", draft({ kind: "evonode", nodeId: "x", p2pPort: "70000" }), ports, [], null)).toEqual({ nodeId: t(`${R}.error.nodeId`), p2pPort: t(`${R}.error.port`) });
    expect(stepErrors("operator", draft({ kind: "evonode", nodeId: "e".repeat(40), p2pPort: "1", httpPort: "1" }), ports, [], null)).toEqual({ httpPort: t(`${R}.error.samePorts`) });
    expect(stepErrors("keys", draft({ voting: "address", votingAddress: "yOwnerAddress" }), ports, [], "yOwnerAddress")).toEqual({ votingAddress: t(`${R}.error.votingIsOwner`) });
    expect(stepErrors("keys", draft({ payout: "address" }), ports, [], null)).toEqual({ payoutAddress: t(`${R}.error.address`) });
  });

  it("lists the coins of the collateral amount for the type, free ones first", () => {
    const coins = [coin("a-0", { userLocked: true }), coin("b-0"), coin("c-0", { kind: "evonode", amount: 4000 * COIN }), coin("d-0", { amount: 999 * COIN })];
    expect(coinsFor(coins, "regular").map((c) => c.outpoint)).toEqual(["b-0", "a-0"]);
    expect(coinsFor(coins, "evonode").map((c) => c.outpoint)).toEqual(["c-0"]);
  });
});

describe("the register grant (D5)", () => {
  it("caps at the confirmed total plus max(fee, 10,000 duffs)", () => {
    expect(registerCap(preview())).toBe(453 + 10_000);
    expect(registerCap(preview({ fee: 25_000, totalDebit: 25_000 }))).toBe(50_000);
  });

  it("treats any changed figure or term on the re-read as a change", () => {
    expect(samePreview(preview(), preview())).toBe(true);
    expect(samePreview(preview(), preview({ fee: 454, totalDebit: 454 }))).toBe(false);
    expect(samePreview(preview(), preview({ terms: { ...preview().terms, payoutAddress: "yOther" } }))).toBe(false);
  });
});

describe("refusals go to their step and field (§1.6)", () => {
  const refused = (reason: string) => refusalOf(err("masternodes.protx_refused", { kind: "reason", reason }));
  it("maps Core's reasons", () => {
    expect(refused("bad-protx-dup-netinfo-entry")).toEqual({ step: "operator", field: "service", text: t(`${R}.refused.dupService`) });
    expect(refused("bad-protx-dup-key")).toEqual({ step: "operator", field: "operatorKey", text: t(`${R}.refused.dupKey`) });
    expect(refused("protx-dup")).toEqual({ step: "operator", field: null, text: t(`${R}.refused.pendingDup`) });
    expect(refused("bad-protx-payee-reuse")).toEqual({ step: "keys", field: "payoutAddress", text: t(`${R}.refused.payeeReuse`) });
    expect(refused("bad-protx-something")?.step).toBe("review");
  });

  it("names the role of exhausted keys, 20 for owner and voting, 100 for operator and node", () => {
    expect(refusalOf(err("masternodes.keys_exhausted", { kind: "keyRole", role: "voting" }))).toEqual({ step: "keys", field: null, text: t(`${R}.refused.exhausted20`, { role: t(`${R}.role.voting`) }) });
    expect(refusalOf(err("masternodes.keys_exhausted", { kind: "keyRole", role: "platform_node" }))).toEqual({
      step: "operator",
      field: null,
      text: t(`${R}.refused.exhausted100`, { role: t(`${R}.role.platform_node`) }),
    });
  });

  it("maps collateral and signature refusals, and leaves others to the caller", () => {
    expect(refusalOf(err("masternodes.collateral_unavailable"))?.field).toBe("outpoint");
    expect(refusalOf(err("masternodes.signature_mismatch"))).toEqual({ step: "prove", field: "signature", text: t(`${R}.refused.signatureMismatch`) });
    expect(refusalOf(err("invalid_argument", null, "the collateral is this wallet's: register it as an existing coin"))?.step).toBe("collateral");
    expect(refusalOf(err("invalid_argument", null, "something else"))).toBeNull();
    expect(refusalOf(err("send.amount_exceeds_balance"))).toBeNull();
    expect(refusalOf(new Error("x"))).toBeNull();
  });
});

describe("sending follows the network (D14)", () => {
  it("is off on mainnet and while the network is still loading, and on for testnet, regtest and devnet", () => {
    expect(registrationSends(null)).toBe(false);
    expect(registrationSends(undefined)).toBe(false);
    expect(registrationSends({ kind: "mainnet" })).toBe(false);
    expect(registrationSends({ kind: "testnet" })).toBe(true);
    expect(registrationSends({ kind: "regtest" })).toBe(true);
    expect(registrationSends({ kind: "devnet" })).toBe(true);
  });

  it("has no Register at Review or Prove on mainnet", () => {
    const sends = registrationSends({ kind: "mainnet" });
    expect(finalAction("review", false, sends)).toBe("sendOff");
    expect(finalAction("prove", true, sends)).toBe("sendOff");
    expect(finalAction("review", true, sends)).toBe("sendOff");
    expect(finalAction("keys", false, sends)).toBe("continue");
  });

  it("offers Register on testnet", () => {
    const sends = registrationSends({ kind: "testnet" });
    expect(finalAction("review", false, sends)).toBe("register");
    expect(finalAction("prove", true, sends)).toBe("submit");
    expect(finalAction("review", true, sends)).toBe("continue");
  });
});

describe("a held wizard only reads (D14)", () => {
  const backend = () => {
    const b = Object.fromEntries([...HELD_CALLS, ...SENDING_CALLS].map((k) => [k, vi.fn(async () => null)]));
    return b as unknown as Backend & Record<string, ReturnType<typeof vi.fn>>;
  };

  it("reaches the backend with keys and preview, and with nothing else, on mainnet", async () => {
    expect(registrationSends({ kind: "mainnet" })).toBe(false);
    const b = backend();
    const calls = registrationCalls(b, registrationSends({ kind: "mainnet" })) as unknown as Record<string, (...a: unknown[]) => Promise<unknown>>;
    for (const k of HELD_CALLS) await calls[k]("w");
    for (const k of SENDING_CALLS) await expect(calls[k]("w")).rejects.toThrow();
    for (const k of HELD_CALLS) expect(b[k]).toHaveBeenCalledOnce();
    for (const k of SENDING_CALLS) expect(b[k]).not.toHaveBeenCalled();
  });

  it("passes every call through on testnet", async () => {
    const b = backend();
    const calls = registrationCalls(b, registrationSends({ kind: "testnet" })) as unknown as Record<string, (...a: unknown[]) => Promise<unknown>>;
    for (const k of SENDING_CALLS) await calls[k]("w");
    for (const k of SENDING_CALLS) expect(b[k]).toHaveBeenCalledOnce();
  });

  it("is the only way the masternode screens reach a registration's sending calls", () => {
    const dir = join(__dirname, "../screens/masternodes");
    const named = SENDING_CALLS.filter((k) => k !== "mnResendOperation");
    for (const f of readdirSync(dir).filter((x) => /\.tsx?$/.test(x) && !x.includes(".test."))) {
      const src = readFileSync(join(dir, f), "utf8");
      const gated = /const b = registrationCalls\(/.test(src);
      for (const k of named) {
        expect(src.match(new RegExp(`\\bbackend\\.${k}\\(`)), `${f} calls ${k} on the backend`).toBeNull();
        if (!gated) expect(src.match(new RegExp(`\\bb\\.${k}\\(`)), `${f} calls ${k} on the backend`).toBeNull();
      }
    }
    expect(readFileSync(join(dir, "RegisterSheet.tsx"), "utf8")).toMatch(/const b = registrationCalls\(backend, sends\);/);
  });
});

describe("Review's collateral line", () => {
  it.each([
    ["fund_new", "Locked against spending while the masternode is registered"],
    ["existing", "Locked against spending while the masternode is registered"],
    ["external", "Keep it unspent in the wallet that holds it. Spending it removes this masternode from the masternode list."],
  ] as const)("%s", (pick, line) => {
    expect(t(collateralNote(pick))).toBe(line);
  });
});
