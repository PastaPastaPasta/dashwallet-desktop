import { describe, expect, it } from "vitest";
import type { ShieldedStatus } from "../api/generated/ShieldedStatus";
import { SHIELDED_SCAN_POLL_MS, shieldedStatusRefetchMs } from "../api/queries";
import { shieldedReceiveGate, shieldedScanPercent, shieldedSyncing, shieldedSyncText, syncingRefusal } from "./syncing";

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

  it("says Scanning… while a pass runs, else when the last pass ended", () => {
    const now = 1_800_000_000;
    expect(shieldedSyncText(status({ syncing: true, syncedIndex: 4096, lastSyncAt: now - 600 }), now)).toBe("Scanning…");
    expect(shieldedSyncText(status({ syncing: true, ready: false, syncedIndex: null, lastSyncAt: null }), now)).toBe("Scanning…");
    expect(shieldedSyncText(status({ syncedIndex: 4096, lastSyncAt: now - 20 }), now)).toBe("Last checked just now");
    expect(shieldedSyncText(status({ syncedIndex: 4096, lastSyncAt: now - 300 }), now)).toBe("Last checked 5 min ago");
    expect(shieldedSyncText(status({ syncedIndex: 4096, lastSyncAt: now - 3 * 3600 }), now)).toBe("Last checked 3 hours ago");
    // No completed pass yet, Shielded off, or not read: no line.
    expect(shieldedSyncText(status({ syncedIndex: null, lastSyncAt: null }), now)).toBeNull();
    expect(shieldedSyncText(status({ active: false, syncing: true }), now)).toBeNull();
    expect(shieldedSyncText(null, now)).toBeNull();
  });

  it("re-reads the status every 5 s while a pass runs, and stops once it has ended", () => {
    expect(SHIELDED_SCAN_POLL_MS).toBe(5_000);
    expect(shieldedStatusRefetchMs(status({ syncing: true }))).toBe(5_000);
    expect(shieldedStatusRefetchMs(status({ syncing: false }))).toBe(false);
    expect(shieldedStatusRefetchMs(null)).toBe(false);
    expect(shieldedStatusRefetchMs(undefined)).toBe(false);
  });

  it("re-reads it while the account is not established and only time holds it, not an unlock", () => {
    expect(shieldedStatusRefetchMs(status({ ready: false, account: "bound" }))).toBe(5_000);
    expect(shieldedStatusRefetchMs(status({ ready: false, account: "binding" }))).toBe(15_000);
    expect(shieldedStatusRefetchMs(status({ ready: false, syncing: true, account: "binding" }))).toBe(15_000);
    expect(shieldedStatusRefetchMs(status({ ready: false, account: "locked" }))).toBe(false);
    expect(shieldedStatusRefetchMs(status({ ready: false, account: "mixingOnly" }))).toBe(false);
    expect(shieldedStatusRefetchMs(status({ ready: false, account: "watchOnly" }))).toBe(false);
    expect(shieldedStatusRefetchMs(status({ active: false, ready: false, account: null }))).toBe(false);
  });

  it("shows the address once the key is bound, a wait while it binds, and why there is none", () => {
    expect(shieldedReceiveGate(status({ ready: false, account: "bound" }))).toBe("address");
    expect(shieldedReceiveGate(status({ account: "bound" }))).toBe("address");
    expect(shieldedReceiveGate(status({ ready: false, account: "binding" }))).toBe("binding");
    expect(shieldedReceiveGate(status({ ready: false, account: "locked" }))).toBe("unlock");
    expect(shieldedReceiveGate(status({ ready: false, account: "mixingOnly" }))).toBe("unlock");
    expect(shieldedReceiveGate(status({ ready: false, account: "watchOnly" }))).toBe("watchOnly");
  });

  it("gives the first scan's share of the pool, never 100%, and none once established", () => {
    expect(shieldedScanPercent(status({ ready: false, scan: { checked: 25, total: 100 } }))).toBe(25);
    expect(shieldedScanPercent(status({ ready: false, scan: { checked: 100, total: 100 } }))).toBe(99.9);
    expect(shieldedScanPercent(status({ ready: false, scan: null }))).toBeNull();
    expect(shieldedScanPercent(status({ ready: false, scan: { checked: 0, total: 0 } }))).toBeNull();
    expect(shieldedScanPercent(status({ ready: true, scan: { checked: 25, total: 100 } }))).toBeNull();
  });
});
