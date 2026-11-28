// The status bar waits on every sync the user waits on: the network, Platform and Shielded. It says
// Synced only when none is syncing, unfinished or unread, and a sync it does not wait on (an unlock,
// a watch-only wallet) says why in Sync progress.

import { describe, expect, it } from "vitest";
import type { DashPaySyncStatus } from "../api/generated/DashPaySyncStatus";
import type { ShieldedSyncInfo } from "../api/generated/ShieldedSyncInfo";
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

const shielded = (over: Partial<ShieldedSyncInfo> = {}): ShieldedSyncInfo => ({
  active: true,
  account: "bound",
  established: true,
  syncing: false,
  scan: null,
  lastPassFailed: false,
  spendsAvailable: true,
  ...over,
});

const platform = (over: Partial<DashPaySyncStatus> = {}): DashPaySyncStatus => ({
  startup: "ready",
  lastPass: null,
  pendingContactCrypto: 0,
  loops: [{ syncLoop: "platformAddressSync", running: true, lastRunAt: 10, nextRunAt: 70 }],
  quorumSource: "spv",
  platformEnabled: true,
  addresses: "done",
  ...over,
});

const bar = (p: DashPaySyncStatus | null, s: ShieldedSyncInfo | null, sync: SyncStatus = caughtUp) => barSync(sync, [coreItem(sync), platformItem(p), shieldedItem(s)]);
const row = (i: SyncItem | null) => itemText(i!, (p) => percentText(p, "en-US"));

describe("the status bar", () => {
  it("says Synced when the network, Platform and Shielded are done", () => {
    expect(bar(platform(), shielded())).toEqual({ text: "Up to date", percent: null, button: "synced" });
  });

  it("says Syncing Shielded with the first scan's share while the account is not established", () => {
    const s = shielded({ established: false, syncing: true, scan: { checked: 42_000, total: 100_000 } });
    expect(bar(platform(), s)).toEqual({ text: "Syncing Shielded…", percent: 42, button: "syncing" });
    expect(row(shieldedItem(s))).toBe("42.0% scanned");
    // Before the pass knows the pool's size: no share.
    expect(bar(platform(), shielded({ established: false, scan: null }))).toEqual({ text: "Syncing Shielded…", percent: null, button: "syncing" });
  });

  it("never reads 100% before the first scan ends", () => {
    const s = shielded({ established: false, syncing: true, scan: { checked: 100_000, total: 100_000 } });
    expect(bar(platform(), s).percent).toBe(99.9);
  });

  // Review: a running pass shows, ahead of the last pass's outcome.
  it("says Syncing Shielded while an established account's pass runs, ahead of a failed last pass", () => {
    const retry = shielded({ syncing: true, lastPassFailed: true, scan: { checked: 99_000, total: 100_000 } });
    expect(bar(platform(), retry)).toEqual({ text: "Syncing Shielded…", percent: 99, button: "syncing" });
    expect(bar(platform(), shielded({ syncing: true })).button).toBe("syncing");
    const passes = [shielded({ lastPassFailed: true }), retry, shielded()];
    expect(passes.map((s) => bar(platform(), s).button)).toEqual(["incomplete", "syncing", "synced"]);
  });

  it("says Syncing Platform while the bring-up runs or the wallet's first address pass has not run", () => {
    expect(bar(platform({ startup: "starting" }), shielded())).toEqual({ text: "Syncing Platform…", percent: null, button: "syncing" });
    expect(bar(platform({ addresses: "pending" }), shielded())).toEqual({ text: "Syncing Platform…", percent: null, button: "syncing" });
    expect(row(platformItem(platform({ addresses: "pending" })))).toBe("Syncing…");
  });

  // Review: Platform is done only once the wallet's own address pass succeeded, not after any pass.
  it("never says Platform is done while the wallet's address pass failed, until one succeeds", () => {
    const failed = platform({ addresses: "failed" });
    expect(bar(failed, shielded())).toEqual({ text: "Platform sync didn't finish", percent: null, button: "incomplete" });
    expect(row(platformItem(failed))).toBe("The last pass failed. It tries again shortly.");
    expect(["pending", "failed", "done"].map((addresses) => bar(platform({ addresses: addresses as DashPaySyncStatus["addresses"] }), shielded()).button)).toEqual(["syncing", "incomplete", "synced"]);
  });

  it("lists Platform before Shielded, and the network before both", () => {
    const s = shielded({ established: false });
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
    const readings: [DashPaySyncStatus, ShieldedSyncInfo][] = [
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
    expect(bar(platform(), shielded({ established: false, lastPassFailed: true })).text).toBe("Syncing Shielded…");
  });

  it("shows why a Shielded pass was held, even before the account is established", () => {
    const reason = "Shielded sync is paused because imported spend history could not be read or saved.";
    const held = shielded({ established: false, lastPassFailed: true, lastPassFailureReason: reason });
    expect(bar(platform(), held)).toEqual({ text: "Shielded sync didn't finish", percent: null, button: "incomplete" });
    expect(row(shieldedItem(held))).toBe(reason);
    expect(bar(platform(), { ...held, syncing: true }).button).toBe("syncing");
  });

  it("puts a syncing item before one that did not finish", () => {
    expect(bar(platform({ startup: "discoveryFailed" }), shielded({ established: false })).text).toBe("Syncing Shielded…");
  });
});

// Review: a status not read yet, or whose read failed, is never Synced.
describe("a sync whose status is not read", () => {
  const items = (p: SyncItem | null, s: SyncItem | null) => barSync(caughtUp, [coreItem(caughtUp), p, s]);

  it("is syncing until its first reading arrives", () => {
    expect(items(platformItem(undefined), shieldedItem(shielded()))).toEqual({ text: "Syncing Platform…", percent: null, button: "syncing" });
    expect(items(platformItem(platform()), shieldedItem(undefined))).toEqual({ text: "Syncing Shielded…", percent: null, button: "syncing" });
  });

  it("is unread, never Synced, when its read failed with no reading", () => {
    expect(items(platformItem(undefined, true), shieldedItem(shielded()))).toEqual({ text: "Couldn't check Platform's sync", percent: null, button: "incomplete" });
    expect(items(platformItem(platform()), shieldedItem(undefined, true))).toEqual({ text: "Couldn't check Shielded's sync", percent: null, button: "incomplete" });
    expect(row(shieldedItem(undefined, true))).toBe("Couldn't check this sync. It tries again shortly.");
  });

  it("keeps the last reading when a later read fails", () => {
    expect(items(platformItem(platform(), true), shieldedItem(shielded(), true)).button).toBe("synced");
  });
});

describe("a sync the bar does not wait on", () => {
  // MG: Synced with a locked or watch-only Shielded only while Sync progress says why.
  it("is Synced in the bar and Unlock to sync in Sync progress while the first bind needs an unlock", () => {
    const s = shielded({ established: false, account: "locked" });
    expect(bar(platform(), s).button).toBe("synced");
    expect(row(shieldedItem(s))).toBe("Unlock to sync");
    const p = platform({ startup: "identityUnsettled" });
    expect(bar(p, shielded()).button).toBe("synced");
    expect(row(platformItem(p))).toBe("Unlock to sync");
    expect(row(platformItem(p, false, "locked"))).toBe("Unlock to sync");
  });

  // MG: a wallet unlocked for mixing only needs the full unlock, and both rows say so.
  it("says Unlock fully to sync on both rows while the wallet is unlocked for mixing only", () => {
    const s = shielded({ established: false, account: "mixingOnly" });
    expect(bar(platform(), s).button).toBe("synced");
    expect(row(shieldedItem(s))).toBe("Unlock fully to sync");
    const p = platform({ startup: "identityUnsettled" });
    expect(row(platformItem(p, false, "unlockedMixingOnly"))).toBe("Unlock fully to sync");
    expect(barSync(caughtUp, [coreItem(caughtUp), platformItem(p, false, "unlockedMixingOnly"), shieldedItem(s)]).button).toBe("synced");
  });

  // Review: the address pass runs without the full unlock; its outcome outranks the identity wait.
  it("waits on a pending or failed Platform-address pass while DashPay waits for an unlock", () => {
    for (const lock of ["locked", "unlockedMixingOnly"] as const) {
      const pending = platform({ startup: "identityUnsettled", addresses: "pending" });
      expect(row(platformItem(pending, false, lock))).toBe("Syncing…");
      expect(bar(pending, shielded()).button).toBe("syncing");
      const failed = platform({ startup: "identityUnsettled", addresses: "failed" });
      expect(row(platformItem(failed, false, lock))).toBe("The last pass failed. It tries again shortly.");
      expect(bar(failed, shielded())).toEqual({ text: "Platform sync didn't finish", percent: null, button: "incomplete" });
    }
    const done = platform({ startup: "identityUnsettled", addresses: "done" });
    expect(row(platformItem(done, false, "unlockedMixingOnly"))).toBe("Unlock fully to sync");
    expect(bar(done, shielded()).button).toBe("synced");
  });

  it("is Synced in the bar and the watch-only line in Sync progress for a watch-only wallet", () => {
    const s = shielded({ established: false, account: "watchOnly" });
    expect(bar(platform(), s).button).toBe("synced");
    expect(row(shieldedItem(s))).toBe("Not available in a watch-only wallet");
  });

  it("is not listed where Shielded or Platform is not on the network or the session", () => {
    expect(shieldedItem(shielded({ active: false, account: null }))).toBeNull();
    expect(shieldedItem(null)).toBeNull();
    expect(platformItem(null)).toBeNull();
    expect(bar(null, null).button).toBe("synced");
  });

  // Review: with --no-platform no pass runs; neither sync is waited on or shown as done.
  it("is not listed in a session without Platform", () => {
    const p = platform({ platformEnabled: false, startup: "notRun", addresses: "pending" });
    expect(platformItem(p)).toBeNull();
    expect(bar(p, shielded({ active: false, account: null }))).toEqual({ text: "Up to date", percent: null, button: "synced" });
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
