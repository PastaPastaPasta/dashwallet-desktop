// The status bar waits on every sync the user waits on: the network, Platform and Shielded. It says
// Synced only when none is syncing or unfinished, and a sync it does not wait on (an unlock, a
// watch-only wallet) says why in Sync progress.

import { describe, expect, it } from "vitest";
import type { DashPaySyncStatus } from "../api/generated/DashPaySyncStatus";
import type { ShieldedStatus } from "../api/generated/ShieldedStatus";
import type { SyncStatus } from "../api/generated/SyncStatus";
import { barSync, coreItem, itemText, platformItem, shieldedItem, type SyncItem } from "./syncItems";
import { percentText } from "./sync";

const caughtUp: SyncStatus = {
  running: true,
  phases: [{ phase: "headers", currentHeight: 100, targetHeight: 100, done: true }],
  activePhase: null,
  tipHeight: 100,
  tipTime: null,
  chainlockHeight: null,
  connectedPeers: 8,
  caughtUp: true,
  secondsSinceProgress: null,
};

const shielded = (over: Partial<ShieldedStatus> = {}): ShieldedStatus => ({
  active: true,
  protocolVersion: 13,
  activationVersion: 12,
  ready: true,
  balance: "0",
  pending: "0",
  pendingShields: [],
  unresolvedSpends: [],
  unrecordedSpends: [],
  syncedIndex: 4096,
  lastSyncAt: 1,
  syncing: false,
  proverReady: true,
  account: "bound",
  scan: null,
  lastPassFailed: false,
  ...over,
});

const platform = (over: Partial<DashPaySyncStatus> = {}): DashPaySyncStatus => ({
  startup: "ready",
  lastPass: null,
  pendingContactCrypto: 0,
  loops: [{ syncLoop: "platformAddressSync", running: true, lastRunAt: 10, nextRunAt: 70 }],
  quorumSource: "spv",
  ...over,
});

const bar = (p: DashPaySyncStatus | null, s: ShieldedStatus | null, sync: SyncStatus = caughtUp) => barSync(sync, [coreItem(sync), platformItem(p), shieldedItem(s)]);
const row = (i: SyncItem | null) => itemText(i!, (p) => percentText(p, "en-US"));

describe("the status bar", () => {
  it("says Synced when the network, Platform and Shielded are done", () => {
    expect(bar(platform(), shielded())).toEqual({ text: "Up to date", percent: null, button: "synced" });
  });

  it("says Syncing Shielded with the first scan's share while the account is not established", () => {
    const s = shielded({ ready: false, syncing: true, syncedIndex: null, scan: { checked: 42_000, total: 100_000 } });
    expect(bar(platform(), s)).toEqual({ text: "Syncing Shielded…", percent: 42, button: "syncing" });
    expect(row(shieldedItem(s))).toBe("42.0% scanned");
    // Before the pass knows the pool's size: no share.
    expect(bar(platform(), shielded({ ready: false, scan: null }))).toEqual({ text: "Syncing Shielded…", percent: null, button: "syncing" });
  });

  it("never reads 100% before the first scan ends", () => {
    const s = shielded({ ready: false, syncing: true, scan: { checked: 100_000, total: 100_000 } });
    expect(bar(platform(), s).percent).toBe(99.9);
  });

  it("says Syncing Platform while the bring-up runs or the addresses' first pass has not run", () => {
    expect(bar(platform({ startup: "starting" }), shielded())).toEqual({ text: "Syncing Platform…", percent: null, button: "syncing" });
    expect(bar(platform({ loops: [{ syncLoop: "platformAddressSync", running: true, lastRunAt: null, nextRunAt: null }] }), shielded()).button).toBe("syncing");
    // Stopped loops are not waited on (no Platform on this network, or SPV not running).
    expect(bar(platform({ loops: [{ syncLoop: "platformAddressSync", running: false, lastRunAt: null, nextRunAt: null }] }), shielded()).button).toBe("synced");
  });

  it("lists Platform before Shielded, and the network before both", () => {
    const s = shielded({ ready: false });
    expect(bar(platform({ startup: "starting" }), s).text).toBe("Syncing Platform…");
    const behind = { ...caughtUp, caughtUp: false, phases: [{ phase: "headers" as const, currentHeight: 50, targetHeight: 100, done: false }] };
    expect(bar(platform({ startup: "starting" }), s, behind)).toEqual({ text: "Synchronizing with network…", percent: 50, button: "syncing" });
    expect(bar(platform(), s, { ...caughtUp, running: false })).toEqual({ text: "Not syncing", percent: null, button: "offline" });
  });

  it("never says Synced while a Platform bring-up stopped short", () => {
    for (const startup of ["discoveryFailed", "identityScanIncomplete", "partialNoIdentity", "partialAccountsPending", "seedBindingUnverified"] as const) {
      const p = platform({ startup });
      expect(bar(p, shielded())).toEqual({ text: "Platform sync didn't finish", percent: null, button: "incomplete" });
      expect(row(platformItem(p))).toBe("Didn't finish. It tries again the next time you start Dash Wallet.");
    }
  });

  it("never says Synced after an established account's last pass failed, until one succeeds", () => {
    expect(bar(platform(), shielded({ lastPassFailed: true }))).toEqual({ text: "Shielded sync didn't finish", percent: null, button: "incomplete" });
    expect(row(shieldedItem(shielded({ lastPassFailed: true })))).toBe("The last pass failed. It tries again shortly.");
    expect(bar(platform(), shielded({ lastPassFailed: false })).button).toBe("synced");
  });

  // MG: after a successful retry the bar leaves Incomplete on its own, from the next reading.
  it("leaves Incomplete once the retry succeeds, for Platform and for Shielded", () => {
    const readings: [DashPaySyncStatus, ShieldedStatus][] = [
      [platform({ startup: "discoveryFailed" }), shielded()],
      [platform({ startup: "starting" }), shielded()],
      [platform(), shielded()],
    ];
    expect(readings.map(([p, s]) => bar(p, s).button)).toEqual(["incomplete", "syncing", "synced"]);
    const passes = [shielded({ lastPassFailed: true }), shielded({ lastPassFailed: false })];
    expect(passes.map((s) => bar(platform(), s).button)).toEqual(["incomplete", "synced"]);
    expect(row(shieldedItem(passes[1]))).toBe("Done");
  });

  it("still says Syncing Shielded when a pass fails before the account is established", () => {
    expect(bar(platform(), shielded({ ready: false, lastPassFailed: true })).text).toBe("Syncing Shielded…");
  });

  it("puts a syncing item before one that did not finish", () => {
    expect(bar(platform({ startup: "discoveryFailed" }), shielded({ ready: false })).text).toBe("Syncing Shielded…");
  });
});

describe("a sync the bar does not wait on", () => {
  // MG: Synced with a locked or watch-only Shielded only while Sync progress says why.
  it("is Synced in the bar and Unlock to sync in Sync progress while the first bind needs an unlock", () => {
    for (const account of ["locked", "mixingOnly"] as const) {
      const s = shielded({ ready: false, account, syncedIndex: null });
      expect(bar(platform(), s).button).toBe("synced");
      expect(row(shieldedItem(s))).toBe("Unlock to sync");
    }
    const p = platform({ startup: "identityUnsettled" });
    expect(bar(p, shielded()).button).toBe("synced");
    expect(row(platformItem(p))).toBe("Unlock to sync");
  });

  it("is Synced in the bar and the watch-only line in Sync progress for a watch-only wallet", () => {
    const s = shielded({ ready: false, account: "watchOnly", syncedIndex: null });
    expect(bar(platform(), s).button).toBe("synced");
    expect(row(shieldedItem(s))).toBe("Not available in a watch-only wallet");
  });

  it("is not listed where Shielded or Platform is not on the network", () => {
    expect(shieldedItem(shielded({ active: false, account: null }))).toBeNull();
    expect(shieldedItem(null)).toBeNull();
    expect(platformItem(undefined)).toBeNull();
    expect(bar(null, null).button).toBe("synced");
  });
});

describe("the network's row", () => {
  it("is done, syncing, not syncing or waiting", () => {
    expect(row(coreItem(caughtUp))).toBe("Done");
    expect(row(coreItem({ ...caughtUp, caughtUp: false }))).toBe("Syncing…");
    expect(row(coreItem({ ...caughtUp, running: false }))).toBe("Not syncing");
    expect(row(coreItem(undefined))).toBe("Waiting");
  });
});
