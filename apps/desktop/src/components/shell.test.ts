import { describe, expect, it } from "vitest";
import type { IdentitySummary } from "../api/generated/IdentitySummary";
import { chipState } from "./shell";

const alice = (mainName: string | null, namesUpdating = false) => ({ identity: "A", mainName, namesUpdating }) as IdentitySummary;

describe("identity chip (DASHPAY §4, U-01 D1)", () => {
  it("F1 follows the DashPay status: Join, Registering, @name · Pending, the main identity", () => {
    expect(chipState({ kind: "noIdentity", reason: null }, undefined)).toEqual({ kind: "join" });
    expect(chipState({ kind: "noIdentity", reason: "waitingForSync" }, undefined)).toEqual({ kind: "join" });
    expect(chipState({ kind: "registering", draft: "1" }, undefined)).toEqual({ kind: "registering", label: null });
    expect(chipState({ kind: "contestPending", identity: "A", label: "alice", endsAt: null }, alice("alice007"))).toEqual({ kind: "registering", label: "alice" });
    expect(chipState({ kind: "ready", main: "A" }, alice("alice"))).toEqual({ kind: "identity", username: "alice", updating: false });
    // DashPay starting, or a status that cannot be read: no chip.
    expect(chipState({ kind: "startupIncomplete", startup: "notRun" }, undefined)).toBeNull();
    expect(chipState(undefined, undefined)).toBeNull();
  });

  it("BL-153 D12: a nameless identity says so; only unknown names are 'updating'", () => {
    expect(chipState({ kind: "ready", main: "A" }, alice(null))).toEqual({ kind: "identity", username: null, updating: false });
    expect(chipState({ kind: "ready", main: "A" }, alice(null, true))).toEqual({ kind: "identity", username: null, updating: true });
    expect(chipState({ kind: "ready", main: "A" }, undefined)).toEqual({ kind: "identity", username: null, updating: true });
  });
});
