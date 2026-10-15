import { describe, expect, it } from "vitest";
import type { IdentitySummary } from "../api/generated/IdentitySummary";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import { chipState } from "./shell";

const alice = (mainName: string | null, namesUpdating = false, namesCheck: IdentitySummary["namesCheck"] = null) =>
  ({ identity: "A", mainName, namesUpdating, namesCheck }) as IdentitySummary;

describe("identity chip (DASHPAY §4, U-01 D1)", () => {
  it("F1 follows the DashPay status: Join, Registering, @name · Pending, the main identity", () => {
    expect(chipState({ kind: "noIdentity", reason: null }, undefined)).toEqual({ kind: "join" });
    expect(chipState({ kind: "noIdentity", reason: "waitingForSync" }, undefined)).toEqual({ kind: "join" });
    expect(chipState({ kind: "registering", draft: "1" }, undefined)).toEqual({ kind: "registering", label: null });
    expect(chipState({ kind: "contestPending", identity: "A", label: "alice", endsAt: null }, alice("alice007"))).toEqual({ kind: "contest", label: "alice" });
    expect(chipState({ kind: "ready", main: "A" }, alice("alice"))).toEqual({ kind: "identity", username: "alice", names: "named" });
    // DashPay starting, or a status that cannot be read: no chip.
    expect(chipState({ kind: "startupIncomplete", startup: "notRun" }, undefined)).toBeNull();
    expect(chipState(undefined, undefined)).toBeNull();
  });

  it("BL-153 D12, DP1-08 D23: a nameless identity says so only once its names were fetched", () => {
    const ready = { kind: "ready", main: "A" } as const;
    expect(chipState(ready, alice(null))).toEqual({ kind: "identity", username: null, names: "none" });
    expect(chipState(ready, alice(null, true))).toEqual({ kind: "identity", username: null, names: "updating" });
    expect(chipState(ready, undefined)).toEqual({ kind: "identity", username: null, names: "updating" });
    expect(chipState(ready, alice(null, false, "pending"))).toEqual({ kind: "identity", username: null, names: "updating" });
    expect(chipState(ready, alice(null, false, "failed"))).toEqual({ kind: "identity", username: null, names: "unknown" });
  });

  it("DP1-08: a watch-only wallet has no chip; a registration names its username once its row is read", () => {
    expect(chipState({ kind: "noIdentity", reason: "watchOnly" }, undefined)).toBeNull();
    const row = { draft: "1", label: "bob" } as RegistrationStatus;
    expect(chipState({ kind: "registering", draft: "1" }, undefined, row)).toEqual({ kind: "registering", label: "bob" });
    // Another draft's row says nothing about this one.
    expect(chipState({ kind: "registering", draft: "2" }, undefined, row)).toEqual({ kind: "registering", label: null });
  });
});
