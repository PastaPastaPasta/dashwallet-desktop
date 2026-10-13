import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installFixture } from "../fixture";
import type { FlowLockReport } from "./generated/FlowLockReport";
import type { LockOutcome } from "./generated/LockOutcome";
import { reconcileLockReport, reportsAnything, summarizeLockReport } from "./lockReport";
import { IpcBackend } from "./tauri";

const W = "a1".repeat(32);
const flow = (outcome: FlowLockReport["outcome"], artifactsOmitted = 0): FlowLockReport => ({
  lease: `l-${outcome}`,
  walletId: W,
  flow: "topUp",
  outcome,
  artifacts: [{ artifact: `a-${outcome}`, outcome }],
  artifactsOmitted,
});
const report = (flows: FlowLockReport[], flowsOmitted = 0, walletsWithSecretsOmitted = 0): LockOutcome => ({
  status: { state: "locked", encrypted: true, quickUnlockEnrolled: false, failedAttempts: 0, retryAfterSecs: null, walletsWithSecrets: [W] },
  flows,
  flowsOmitted,
  walletsWithSecretsOmitted,
});

// E0-13 D3 (Sol r4 Low 1): a report with any omitted count above 0 is not complete, and the
// renderer treats what it leaves out as unresolved.
describe("lock report consumer", () => {
  it("a complete report of ended flows is resolved", () => {
    const s = summarizeLockReport(report([flow("sent"), flow("cancelled")]));
    expect(s).toMatchObject({ complete: true, unresolved: false, pending: [] });
  });

  it("a listed pending flow is unresolved", () => {
    expect(summarizeLockReport(report([flow("maybeSent")])).unresolved).toBe(true);
    expect(summarizeLockReport(report([flow("willBeSent")])).unresolved).toBe(true);
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

  it("an empty complete report shows nothing", () => {
    expect(reportsAnything(report([]))).toBe(false);
  });

  describe("reconciliation through leases, dispatchStatus and lockStatus", () => {
    beforeEach(() => vi.stubGlobal("window", globalThis));
    afterEach(() => {
      clearMocks();
      vi.unstubAllGlobals();
    });

    it("the omitted-counts fixture stays unresolved until no lease may still send", async () => {
      const fixture = installFixture({ scenario: "lock-report-omitted" });
      const b = new IpcBackend("fixture");
      const outcome = await b.lock();
      const summary = summarizeLockReport(outcome);
      expect(summary.omitted).toEqual({ flows: 3, artifacts: 7, wallets: 2 });
      expect(summary.unresolved).toBe(true);

      let r = await reconcileLockReport(outcome, b);
      expect(r.settled).toBe(false);
      expect(r.artifacts.map((a) => a.state)).toEqual(["maybeSent", "willBeSent"]);

      // Every listed artifact settles, but leases still hold committed funds: what the
      // report left out may still be sent.
      fixture.world.dispatch["tx:3f9a1c27"] = "notSent";
      fixture.world.dispatch["st:8be04d11"] = "sent";
      r = await reconcileLockReport(outcome, b);
      expect(r.settled).toBe(false);
      expect(r.live.length).toBeGreaterThan(0);

      fixture.world.leases = fixture.world.leases.map((l) => ({ ...l, state: { kind: "ended" }, inFlight: 0, callRunning: false }));
      r = await reconcileLockReport(outcome, b);
      expect(r.settled).toBe(true);
      // lockStatus lists the wallets the report cut.
      expect(r.lock.walletsWithSecrets).toEqual(fixture.world.lock.walletsWithSecrets);
    });

    it("a complete report settles once its pending artifacts do", async () => {
      const fixture = installFixture({ scenario: "lock-report-pending" });
      const b = new IpcBackend("fixture");
      const outcome = await b.lock();
      expect(summarizeLockReport(outcome)).toMatchObject({ complete: true, unresolved: true });
      expect((await reconcileLockReport(outcome, b)).settled).toBe(false);
      fixture.world.dispatch["tx:3f9a1c27"] = "sent";
      fixture.world.dispatch["st:8be04d11"] = "sent";
      expect((await reconcileLockReport(outcome, b)).settled).toBe(true);
    });
  });
});
