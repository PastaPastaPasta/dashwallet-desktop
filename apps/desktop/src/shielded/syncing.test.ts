import { describe, expect, it } from "vitest";
import type { ShieldedStatus } from "../api/generated/ShieldedStatus";
import { shieldedSyncing, shieldedSyncText, syncingRefusal } from "./syncing";

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

  it("says Scanning… while a pass runs, else when the last completed pass ended", () => {
    const now = 1_800_000_000;
    expect(shieldedSyncText(status({ syncing: true, syncedIndex: 4096, lastSyncAt: now - 600 }), now)).toBe("Scanning…");
    expect(shieldedSyncText(status({ syncing: true, ready: false, syncedIndex: null, lastSyncAt: null }), now)).toBe("Scanning…");
    expect(shieldedSyncText(status({ syncedIndex: 4096, lastSyncAt: now - 20 }), now)).toBe("Last synced just now");
    expect(shieldedSyncText(status({ syncedIndex: 4096, lastSyncAt: now - 300 }), now)).toBe("Last synced 5 min ago");
    expect(shieldedSyncText(status({ syncedIndex: 4096, lastSyncAt: now - 3 * 3600 }), now)).toBe("Last synced 3 hours ago");
    // No completed pass yet, Shielded off, or not read: no line.
    expect(shieldedSyncText(status({ syncedIndex: null, lastSyncAt: null }), now)).toBeNull();
    expect(shieldedSyncText(status({ active: false, syncing: true }), now)).toBeNull();
    expect(shieldedSyncText(null, now)).toBeNull();
  });
});
