// The fixture's username registration (DP1-09) over the real IPC path: the start's refusals, a
// run step by step, a relaunch that keeps the row and drops the run, and a resume that sends no
// second asset lock; names from credits; the name checks.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import { coreRequest } from "../dashpay/wizard";
import { IDENTITY } from "./dashpay";
import { installFixture, type FixtureHandle } from "./index";
import { checkUsername } from "./registration";
import { FIXTURE_WALLET_ID, SCENARIO_NAMES, buildWorld, type ScenarioName } from "./scenarios";

const W = FIXTURE_WALLET_ID;
const LABEL = "alice-77";

/** A tab's sessionStorage, which `relaunch` writes and the next install reads. */
function fakeSession() {
  const items = new Map<string, string>();
  return {
    getItem: (k: string) => items.get(k) ?? null,
    setItem: (k: string, v: string) => void items.set(k, v),
    removeItem: (k: string) => void items.delete(k),
  };
}

beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  vi.stubGlobal("sessionStorage", fakeSession());
  vi.stubGlobal("location", { reload: () => undefined });
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

const install = (scenario: ScenarioName = "dashpay-no-identity"): FixtureHandle => installFixture({ scenario });

async function grant(maxDuffs = 3_002_260, maxCredits = "2000000000") {
  return (await cmd.authorize({ kind: "platformOp", maxDuffs, maxCredits }, W, { kind: "passphrase", passphrase: "demo" })).id;
}

const rowOf = async (draft: string) => (await cmd.registrations(W)).find((r) => r.draft === draft)!;

/** Quits and relaunches: the next install reads the world `relaunch` left. */
function relaunch(f: FixtureHandle): FixtureHandle {
  f.relaunch();
  clearMocks();
  return install();
}

describe("start_registration", () => {
  it("writes a funded row, broadcasts one asset lock and says so; the status is registering", async () => {
    const f = install();
    const quote = await cmd.registrationQuote(W, coreRequest(LABEL));
    expect(quote).toMatchObject({ contested: false, totalDuffs: 3_002_260 });
    expect(quote.availableDuffs).toBeGreaterThan(quote.totalDuffs);
    const draft = await cmd.startRegistration(W, coreRequest(LABEL), await grant());
    expect(await rowOf(draft)).toMatchObject({ phase: "fundingSent", label: LABEL, waiting: "instantSend", holdsKey: false, running: true, fundsCommitted: true });
    expect(f.world.dashpay.assetLocks).toBe(1);
    expect(await cmd.dashpayStatus(W)).toEqual({ kind: "registering", draft });
    expect((await cmd.leases()).filter((l) => l.flow === "registration")).toHaveLength(1);
  });

  it("refuses before any row: a bad label, a small grant, too few coins, another registration", async () => {
    const f = install();
    await expect(cmd.startRegistration(W, coreRequest("a"), await grant())).rejects.toMatchObject({ code: "name.invalid", data: { rules: ["minLength"] } });
    await expect(cmd.startRegistration(W, coreRequest(LABEL), await grant(1))).rejects.toMatchObject({ code: "platform.grant_exceeded" });
    const draft = await cmd.startRegistration(W, coreRequest(LABEL), await grant());
    await expect(cmd.startRegistration(W, coreRequest("bob-77"), await grant())).rejects.toMatchObject({ code: "registration.in_progress", data: { draft } });
    expect(f.world.dashpay.assetLocks).toBe(1);

    install("empty");
    await expect(cmd.startRegistration(W, coreRequest(LABEL), await grant())).rejects.toMatchObject({
      code: "registration.funding_insufficient",
      data: { kind: "duffs", needed: 3_002_260, available: 0 },
    });
    expect((await cmd.registrationQuote(W, coreRequest(LABEL))).availableDuffs).toBe(0);
  });

  it("starts past a registration whose funding is unknown only when told to", async () => {
    install("tools-stuck-funding");
    await expect(cmd.startRegistration(W, coreRequest(LABEL), await grant())).rejects.toMatchObject({ code: "registration.in_progress", data: { draft: "reg-1" } });
    const draft = await cmd.startRegistrationPastUnknownFunding(W, coreRequest(LABEL), await grant(), ["reg-1"]);
    expect(draft).not.toBe("reg-1");
  });
});

describe("a run, step by step", () => {
  it("non-contested: identity, name requested, name registered, done; the status ends ready with the name", async () => {
    const f = install();
    const draft = await cmd.startRegistration(W, coreRequest(LABEL), await grant());
    const phases: string[] = [];
    while (f.advanceRegistration(draft)) phases.push((await rowOf(draft)).phase);
    expect(phases).toEqual(["proofWaiting", "identityRegistered", "nameRequested", "nameRegistered", "done"]);
    expect(await cmd.dashpayStatus(W)).toEqual({ kind: "ready", main: IDENTITY.alice });
    expect((await cmd.identities(W))[0]).toMatchObject({ names: [LABEL], mainName: LABEL });
    expect((await cmd.leases()).filter((l) => l.flow === "registration" && l.state.kind === "active")).toEqual([]);
  });

  it("contested: the vote starts and the row ends done; the status is contestPending", async () => {
    const f = install();
    const draft = await cmd.startRegistration(W, coreRequest("alice"), await grant(25_002_260));
    while (f.advanceRegistration(draft));
    expect(await rowOf(draft)).toMatchObject({ phase: "done", contestEndsAt: expect.any(Number) });
    expect(await cmd.dashpayStatus(W)).toMatchObject({ kind: "contestPending", label: "alice" });
  });

  it("a name refused after the identity ends done with nameFailure; a retryable failure keeps the row for Continue", async () => {
    const f = install();
    const draft = await cmd.startRegistration(W, coreRequest(LABEL), await grant());
    f.advanceRegistration(draft);
    f.advanceRegistration(draft);
    f.failRegistration(draft, "name.taken", false);
    expect(await rowOf(draft)).toMatchObject({ phase: "done", nameFailure: "name.taken", identity: IDENTITY.alice });

    const g = install();
    const second = await cmd.startRegistration(W, coreRequest(LABEL), await grant());
    g.failRegistration(second, "platform.unavailable", true);
    expect(await rowOf(second)).toMatchObject({ phase: "failed", failure: { phase: "fundingSent", retryable: true }, waiting: null });
    await cmd.resumeRegistration(W, second, await grant());
    expect(await rowOf(second)).toMatchObject({ phase: "fundingSent", failure: null, waiting: "instantSend" });
    expect(g.world.dashpay.assetLocks).toBe(1);
  });
});

describe("relaunch and resume (the acceptance: no second asset lock)", () => {
  it("the row survives with its funding; the run, grants and leases do not, and the vault opens locked", async () => {
    const f = install();
    const draft = await cmd.startRegistration(W, coreRequest(LABEL), await grant());
    const g = relaunch(f);
    expect(g.world.lock.state).toBe("locked");
    expect(g.world.leases).toEqual([]);
    // No proof was recorded (the fixture models none): the engine's idle_wait says chainLock,
    // locked or not, and no key is held (D21).
    expect(await rowOf(draft)).toMatchObject({ phase: "fundingSent", waiting: "chainLock", holdsKey: false, running: false, fundsCommitted: true });
    expect(g.advanceRegistration(draft)).toBe(false);
    await cmd.unlock("demo");
    expect(await rowOf(draft)).toMatchObject({ waiting: "chainLock", holdsKey: false, running: false });
  });

  it("Continue resumes the same draft and lock, and the run completes", async () => {
    const f = install();
    const draft = await cmd.startRegistration(W, coreRequest(LABEL), await grant());
    const g = relaunch(f);
    await cmd.unlock("demo");
    // No grant: only keyless waits, so it stays idle for the fixture (no proof comes).
    await cmd.resumeRegistration(W, draft, null);
    expect(await rowOf(draft)).toMatchObject({ waiting: "chainLock", holdsKey: false, running: false });
    await cmd.resumeRegistration(W, draft, await grant());
    expect(await rowOf(draft)).toMatchObject({ waiting: "instantSend", holdsKey: false, running: true });
    while (g.advanceRegistration(draft));
    expect(await rowOf(draft)).toMatchObject({ phase: "done" });
    expect(g.world.dashpay.assetLocks).toBe(1);
    expect(g.world.dashpay.registrations).toHaveLength(1);
  });

  // Every scenario's world is built: slow under the box's load, so its own timeout.
  it("every scenario's world survives the trip through JSON", { timeout: 30_000 }, () => {
    for (const name of SCENARIO_NAMES) {
      const w = buildWorld(name);
      expect(JSON.parse(JSON.stringify(w)), name).toEqual(w);
    }
  });
});

describe("a parked run (DP1-09 D14)", () => {
  it("a passphrase change revokes the run's lease: the row waits for Continue, which rebinds it", async () => {
    const f = install();
    const draft = await cmd.startRegistration(W, coreRequest(LABEL), await grant());
    f.world.leases = f.world.leases.map((l) => ({ ...l, state: { kind: "revoked", cause: "passphraseChange" } }));
    // The parked run still runs: Continue hands it the grant (D26).
    expect(await rowOf(draft)).toMatchObject({ waiting: "authorize", holdsKey: false, running: true });
    expect(f.advanceRegistration(draft)).toBe(false);
    await cmd.resumeRegistration(W, draft, await grant());
    expect(await rowOf(draft)).toMatchObject({ waiting: "instantSend", holdsKey: false, running: true });
    expect(f.advanceRegistration(draft)).toBe(true);
    expect(f.world.dashpay.assetLocks).toBe(1);
  });
});

describe("discard and Finish transfers", () => {
  it("D7: an unfunded row is discarded; a funded one is refused", async () => {
    install("dashpay-registering-unlock");
    await cmd.discardRegistration(W, "reg-1");
    expect(await cmd.registrations(W)).toEqual([]);
    expect(await cmd.dashpayStatus(W)).toEqual({ kind: "noIdentity", reason: null });
    install("dashpay-registration-paused");
    await expect(cmd.discardRegistration(W, "reg-1")).rejects.toMatchObject({ code: "invalid_argument" });
  });

  it("D10: Finish transfers resumes the rows waiting for it", async () => {
    install("dashpay-registration-needs-finish");
    expect(await cmd.finishAssetLocks(W, await grant(1))).toEqual({ resumed: 1, completed: 0, stillPending: 0 });
    expect(await rowOf("reg-1")).toMatchObject({ waiting: "instantSend", holdsKey: false, running: true });
  });
});

describe("names", () => {
  it("check_username follows the engine's rules and contested set", () => {
    expect(checkUsername("alice")).toMatchObject({ valid: true, contested: true, normalized: "a11ce" });
    expect(checkUsername("alice-77")).toMatchObject({ valid: true, contested: false });
    expect(checkUsername("a".repeat(20))).toMatchObject({ valid: true, contested: false });
    expect(checkUsername("-a--b").rules.filter((r) => !r.passed).map((r) => r.rule)).toEqual(["noEdgeHyphen", "noDoubleHyphen"]);
    expect(checkUsername("x".repeat(24)).rules.find((r) => r.rule === "maxLength")?.passed).toBe(false);
  });

  it("name_availability: the network's names by homograph, the directory, unknown while offline", async () => {
    const f = install("dashpay-registered");
    expect(await cmd.nameAvailability(W, "B0B")).toEqual({ kind: "taken", owner: IDENTITY.bob });
    expect(await cmd.nameAvailability(W, "carol")).toMatchObject({ kind: "contestOpen" });
    expect(await cmd.nameAvailability(W, "zed-77")).toEqual({ kind: "available", contested: false });
    f.world.dashpay.searchOffline = true;
    expect(await cmd.nameAvailability(W, "zed-77")).toEqual({ kind: "unknown" });
  });

  it("D6: register_name from credits, under grant_request's caps", async () => {
    install("dashpay-no-username");
    const caps = await cmd.grantRequest(W, IDENTITY.alice, { kind: "registerName", label: LABEL });
    expect(caps).toEqual({ maxDuffs: 0, maxCredits: "2000000000" });
    expect(await cmd.registerName(W, IDENTITY.alice, LABEL, await grant(0))).toEqual({ kind: "registered" });
    expect(await cmd.dashpayStatus(W)).toEqual({ kind: "ready", main: IDENTITY.alice });
    await expect(cmd.registerName(W, IDENTITY.alice, "bob", await grant(0))).rejects.toMatchObject({ code: "name.taken" });
  });
});
