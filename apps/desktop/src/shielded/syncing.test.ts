import { describe, expect, it } from "vitest";
import type { ShieldedStatus } from "../api/generated/ShieldedStatus";
import { shieldedSyncing, syncingRefusal } from "./syncing";

const status = (over: Partial<ShieldedStatus>): ShieldedStatus =>
  ({ active: true, ready: true, syncing: false, balance: "0", pending: "0", pendingShields: [], proverReady: true, ...over }) as ShieldedStatus;

describe("Shielded syncing adapter (DEC-1058)", () => {
  it("DEC1058_syncingIsActiveAndNotReady", () => {
    expect(shieldedSyncing(status({ ready: false, syncing: true }))).toBe(true);
    expect(shieldedSyncing(status({ ready: true }))).toBe(false);
    expect(shieldedSyncing(status({ active: false, ready: false }))).toBe(false);
    expect(shieldedSyncing(null)).toBe(false);
    expect(shieldedSyncing(undefined)).toBe(false);
  });

  it("maps its own code always, and a Platform refusal only while syncing", () => {
    expect(syncingRefusal("shielded.not_synced", false)).toBe(true);
    expect(syncingRefusal("platform.unavailable", true)).toBe(true);
    expect(syncingRefusal("platform.unavailable", false)).toBe(false);
    expect(syncingRefusal("shielded.insufficient_funds", true)).toBe(false);
  });
});
