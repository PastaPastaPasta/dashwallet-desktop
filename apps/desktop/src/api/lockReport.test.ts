import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installFixture, type FixtureHandle } from "../fixture";
import type { FlowLockReport } from "./generated/FlowLockReport";
import type { LockOutcome } from "./generated/LockOutcome";
import { isCheckable, reconcileLockReport, reportsAnything, summarizeLockReport } from "./lockReport";
import { IpcBackend } from "./tauri";

const W = "a1".repeat(32);
type Outcome = FlowLockReport["outcome"];
/** A flow with one artifact of its outcome, or the artifacts given. */
const flow = (outcome: Outcome, artifactsOmitted = 0, artifacts: Outcome[] = [outcome]): FlowLockReport => ({
  lease: `l-${outcome}`,
  walletId: W,
  flow: "topUp",
  outcome,
  artifacts: artifacts.map((o, i) => ({ artifact: `topup/${outcome}/${i}`, outcome: o })),
  artifactsOmitted,
});
const report = (flows: FlowLockReport[], flowsOmitted = 0, walletsWithSecretsOmitted = 0): LockOutcome => ({
  status: { state: "locked", encrypted: true, quickUnlockEnrolled: false, failedAttempts: 0, retryAfterSecs: null, walletsWithSecrets: [W] },
  flows,
  flowsOmitted,
  walletsWithSecretsOmitted,
});
const endAllLeases = (f: FixtureHandle) => (f.world.leases = f.world.leases.map((l) => ({ ...l, state: { kind: "ended" }, inFlight: 0, callRunning: false })));

// E0-13 D3 (Sol r4 Low 1): a report with any omitted count above 0 is not complete, and the
// renderer treats what it leaves out as unresolved.
describe("lock report consumer", () => {
  it("a complete report of ended flows is resolved", () => {
    const s = summarizeLockReport(report([flow("sent"), flow("cancelled")]));
    expect(s).toMatchObject({ complete: true, unresolved: false, beyondReach: false, pending: [] });
  });

  it("a listed pending flow is unresolved", () => {
    expect(summarizeLockReport(report([flow("maybeSent")])).unresolved).toBe(true);
    expect(summarizeLockReport(report([flow("willBeSent")])).unresolved).toBe(true);
  });

  // A flow's outcome is the strongest of its artifacts' (E0-04 §8.4): `sent` funding with an
  // identity call still out reads `sent`.
  it("a sent flow holding a pending artifact is unresolved", () => {
    const sentFlow = flow("sent", 0, ["maybeSent", "sent"]);
    const s = summarizeLockReport(report([sentFlow, flow("cancelled")]));
    expect(s).toMatchObject({ complete: true, unresolved: true, beyondReach: false, pending: [sentFlow] });
  });

  it.each([
    ["flowsOmitted", report([flow("sent")], 1)],
    ["artifactsOmitted", report([flow("sent", 1)])],
    ["walletsWithSecretsOmitted", report([flow("sent")], 0, 1)],
    ["flowsOmitted with nothing listed", report([], 5)],
  ])("%s > 0 is unresolved even when every listed flow ended", (_, r) => {
    const s = summarizeLockReport(r);
    expect(s.complete).toBe(false);
    expect(s.unresolved).toBe(true);
    expect(reportsAnything(r)).toBe(true);
  });

  // Listed unresolved first: what follows a pending item may be pending, and has no id to ask about.
  it.each([
    ["flows omitted after a pending flow", report([flow("sent"), flow("maybeSent")], 1), true],
    ["flows omitted after a cancelled flow", report([flow("maybeSent"), flow("cancelled")], 1), false],
    // Flows are listed by outcome: those after a `sent` one may be `sent` with an artifact pending.
    ["flows omitted after a sent flow", report([flow("maybeSent"), flow("sent")], 1), true],
    ["flows omitted with nothing listed", report([], 5), true],
    ["artifacts omitted after a pending one", report([flow("willBeSent", 3, ["willBeSent", "willBeSent"])]), true],
    ["artifacts omitted after a sent one", report([flow("sent", 3, ["willBeSent", "sent"])]), false],
    ["a pending flow with no pending artifact listed", report([flow("maybeSent", 0, [])]), true],
    ["only wallets omitted", report([flow("maybeSent")], 0, 4), false],
    ["a pending call named by its label", report([{ ...flow("sent", 0, ["sent"]), artifacts: [{ artifact: "accept_request", outcome: "maybeSent" }] }]), true],
    ["a settled call named by its label", report([{ ...flow("sent", 0, ["sent"]), artifacts: [{ artifact: "accept_request", outcome: "sent" }] }]), false],
  ])("%s: beyond reach %s", (_, r, beyond) => {
    expect(summarizeLockReport(r).beyondReach).toBe(beyond);
  });

  it("checks only what dispatchStatus can name", () => {
    expect(isCheckable("ab".repeat(32))).toBe(true);
    expect(isCheckable("registration/d1/identity")).toBe(true);
    expect(isCheckable("accept_request")).toBe(false);
    expect(isCheckable("AB".repeat(32))).toBe(false);
    expect(isCheckable("ab".repeat(31))).toBe(false);
  });

  it("an empty complete report shows nothing", () => {
    expect(reportsAnything(report([]))).toBe(false);
  });

  describe("reconciliation through leases, dispatchStatus and lockStatus", () => {
    beforeEach(() => vi.stubGlobal("window", globalThis));
    afterEach(() => {
      clearMocks();
      vi.unstubAllGlobals();
    });

    it("omitted flows after pending ones stay unresolved whatever settles", async () => {
      const fixture = installFixture({ scenario: "lock-report-omitted" });
      const b = new IpcBackend("fixture");
      const outcome = await b.lock();
      const summary = summarizeLockReport(outcome);
      expect(outcome.flows).toHaveLength(16);
      expect(summary.omitted).toEqual({ flows: 3, artifacts: 7, wallets: 2 });
      expect(summary).toMatchObject({ unresolved: true, beyondReach: true });

      // Every listed artifact settles and every lease ends: the 3 omitted flows may still be
      // pending, and nothing names them.
      for (const a of Object.keys(fixture.world.dispatch)) fixture.world.dispatch[a] = "sent";
      endAllLeases(fixture);
      const r = await reconcileLockReport(outcome, b);
      expect(r.artifacts.every((a) => a.state === "sent")).toBe(true);
      expect(r.settled).toBe(false);
    });

    it("omissions the order shows are settled resolve with the listed artifacts", async () => {
      const fixture = installFixture({ scenario: "lock-report-omitted-settles" });
      const b = new IpcBackend("fixture");
      const outcome = await b.lock();
      const summary = summarizeLockReport(outcome);
      expect(summary.omitted).toEqual({ flows: 2, artifacts: 4, wallets: 2 });
      expect(summary).toMatchObject({ complete: false, unresolved: true, beyondReach: false });

      // The top-up reads `sent`, its strongest artifact's, with one still to be sent.
      const [registration, topUp] = outcome.flows;
      expect(topUp.outcome).toBe("sent");
      let r = await reconcileLockReport(outcome, b);
      expect(r.artifacts.map((a) => [a.artifact, a.state])).toEqual([
        [registration.artifacts[0].artifact, "maybeSent"],
        [topUp.artifacts[0].artifact, "willBeSent"],
      ]);
      expect(r.settled).toBe(false);

      fixture.world.dispatch[registration.artifacts[0].artifact] = "notSent";
      fixture.world.dispatch[topUp.artifacts[0].artifact] = "sent";
      r = await reconcileLockReport(outcome, b);
      // The registration's call is still running.
      expect(r.live.map((l) => l.id)).toEqual(["3f9a1c27"]);
      expect(r.settled).toBe(false);

      // The call ends; the revoked leases stay revoked (not ended): they settle nothing either way.
      fixture.world.leases = fixture.world.leases.map((l) => ({ ...l, inFlight: 0, callRunning: false }));
      r = await reconcileLockReport(outcome, b);
      expect(r.settled).toBe(true);
      // lockStatus lists every wallet, the ones the report cut included.
      expect(r.lock.walletsWithSecrets).toEqual(fixture.world.lock.walletsWithSecrets);
    });

    it("a complete report settles once its pending artifacts do", async () => {
      const fixture = installFixture({ scenario: "lock-report-pending" });
      const b = new IpcBackend("fixture");
      const outcome = await b.lock();
      const summary = summarizeLockReport(outcome);
      expect(summary).toMatchObject({ complete: true, unresolved: true, beyondReach: false });
      // The registration's funding was sent; its identity call may have gone out.
      expect(summary.pending.map((f) => [f.flow, f.outcome])).toEqual([
        ["topUp", "willBeSent"],
        ["registration", "sent"],
      ]);
      const r = await reconcileLockReport(outcome, b);
      expect(r.artifacts.map((a) => a.state)).toEqual(["willBeSent", "maybeSent"]);
      expect(r.settled).toBe(false);
      for (const a of r.artifacts) fixture.world.dispatch[a.artifact] = "sent";
      expect((await reconcileLockReport(outcome, b)).settled).toBe(true);
    });

    it("a call named by its label is never asked about, and keeps the report unresolved", async () => {
      const fixture = installFixture({ scenario: "lock-report-call" });
      const b = new IpcBackend("fixture");
      const outcome = await b.lock();
      expect(outcome.flows.map((f) => f.outcome)).toEqual(["sent"]);
      expect(summarizeLockReport(outcome)).toMatchObject({ complete: true, unresolved: true, beyondReach: true });
      endAllLeases(fixture);
      const r = await reconcileLockReport(outcome, b);
      expect(r.artifacts).toEqual([]);
      expect(fixture.calls).not.toContain("dispatch_status");
      expect(r.settled).toBe(false);
      // The engine rejects a label, and so does the fixture.
      await expect(b.dispatchStatus(W, "accept_request")).rejects.toMatchObject({ code: "invalid_argument" });
    });

    it("a failed dispatchStatus leaves its artifact unknown, not the whole check failed", async () => {
      const fixture = installFixture({ scenario: "lock-report-pending" });
      const b = new IpcBackend("fixture");
      const outcome = await b.lock();
      fixture.fail("dispatch_status", "none");
      const r = await reconcileLockReport(outcome, b);
      expect(r.artifacts.map((a) => a.state)).toEqual([null, null]);
      expect(r.settled).toBe(false);
    });
  });
});
