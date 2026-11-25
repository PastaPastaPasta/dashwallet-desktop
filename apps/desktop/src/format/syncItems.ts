// Every sync the user waits on, as the status bar and Sync progress show them: the Dash network
// (SPV), Platform (the DashPay bring-up and the wallet's Platform-address passes) and Shielded (the
// account's passes). The bar says "Synced" only when none is syncing, unfinished or unread; a sync
// that waits for an unlock, or that a watch-only wallet cannot run, is not one the bar waits on,
// and Sync progress says why. A sync whose status was never read counts as syncing, and one whose
// read failed as unread: neither is Synced.

import type { DashPaySyncStatus } from "../api/generated/DashPaySyncStatus";
import type { LockStateName } from "../api/generated/LockStateName";
import type { ShieldedSyncInfo } from "../api/generated/ShieldedSyncInfo";
import type { StartupStatus } from "../api/generated/StartupStatus";
import type { SyncStatus } from "../api/generated/SyncStatus";
import { scanShare, shieldedReceiveGate } from "../shielded/syncing";
import { isSyncing, syncPercent, syncState } from "./sync";
import { t } from "../i18n";

export type SyncItemName = "core" | "platform" | "shielded";

/** `done`; `syncing` (with a percent when known; also a status not read yet); `incomplete`: it
 * stopped short and tries again later; `unread`: its status could not be read; `unlock`: it waits
 * for an unlock; `unlockFully`: for a full unlock, the wallet being unlocked for mixing only;
 * `watchOnly`: this wallet cannot run it; `offline` and `waiting`: the network's own (not running,
 * not read yet). */
export type SyncItemState = "done" | "syncing" | "incomplete" | "unread" | "unlock" | "unlockFully" | "watchOnly" | "offline" | "waiting";

export interface SyncItem {
  name: SyncItemName;
  state: SyncItemState;
  percent: number | null;
  /** `incomplete` for a failed pass that the next one retries shortly, not a bring-up that stopped
   * short until the next start. */
  pass?: boolean;
}

const item = (name: SyncItemName, state: SyncItemState, percent: number | null = null): SyncItem => ({ name, state, percent });

export function coreItem(sync: SyncStatus | undefined): SyncItem {
  if (!sync) return item("core", "waiting");
  if (!sync.running) return item("core", "offline");
  return isSyncing(sync) ? item("core", "syncing") : item("core", "done");
}

/** The bring-up outcomes that stopped short: they try again at the next start (DASHPAY §3.2). */
const PLATFORM_INCOMPLETE: ReadonlySet<StartupStatus> = new Set(["discoveryFailed", "identityScanIncomplete", "partialNoIdentity", "partialAccountsPending", "seedBindingUnverified"]);

/** `null`: Platform is not on this network or this session (`--no-platform`), so not waited on.
 * `failed`: the read failed (`s` is then the last reading, if any). The wallet's address balances
 * are synced once its own Platform-address pass succeeded this session; that pass runs without the
 * full unlock, so its outcome comes before DashPay's wait for one. `lock` tells a wallet unlocked
 * for mixing only from a locked one. */
export function platformItem(s: DashPaySyncStatus | null | undefined, failed = false, lock?: LockStateName): SyncItem | null {
  if (s === null) return null;
  if (s === undefined) return item("platform", failed ? "unread" : "syncing");
  if (!s.platformEnabled) return null;
  if (s.startup === "starting") return item("platform", "syncing");
  if (PLATFORM_INCOMPLETE.has(s.startup)) return item("platform", "incomplete");
  if (s.addresses === "pending") return item("platform", "syncing");
  if (s.addresses === "failed") return { ...item("platform", "incomplete"), pass: true };
  if (s.startup === "identityUnsettled") return item("platform", lock === "unlockedMixingOnly" ? "unlockFully" : "unlock");
  return item("platform", "done");
}

/** `null` while Shielded is not on this network or this session. A running pass is syncing, ahead
 * of the last pass's outcome; not established yet is syncing too (passes run again); once
 * established, a failed last pass is incomplete until one succeeds. */
export function shieldedItem(s: ShieldedSyncInfo | null | undefined, failed = false): SyncItem | null {
  if (s === null) return null;
  if (s === undefined) return item("shielded", failed ? "unread" : "syncing");
  if (!s.active) return null;
  if (s.account === "mixingOnly") return item("shielded", "unlockFully");
  const gate = shieldedReceiveGate(s);
  if (gate === "unlock" || gate === "watchOnly") return item("shielded", gate);
  if (s.syncing) return item("shielded", "syncing", scanShare(s.scan));
  if (!s.established) return item("shielded", "syncing");
  return s.lastPassFailed ? { ...item("shielded", "incomplete"), pass: true } : item("shielded", "done");
}

export type BarButton = "offline" | "syncing" | "incomplete" | "synced";

/** What the status bar shows: its state text, the percent beside it (`null`: none), and the sync
 * button's state. The network's own state comes first: while it is not up to date the bar is as it
 * always was. Then the first item still syncing, then the first that did not finish. */
export function barSync(sync: SyncStatus | undefined, items: readonly (SyncItem | null)[]): { text: string; percent: number | null; button: BarButton | null } {
  if (!sync) return { text: syncState(sync), percent: null, button: null };
  if (!sync.running) return { text: syncState(sync), percent: null, button: "offline" };
  if (isSyncing(sync)) return { text: syncState(sync), percent: syncPercent(sync), button: "syncing" };
  const others = items.filter((i): i is SyncItem => i !== null && i.name !== "core");
  const syncing = others.find((i) => i.state === "syncing");
  if (syncing) return { text: t(syncing.name === "platform" ? "shell.status.syncingPlatform" : "shell.status.syncingShielded"), percent: syncing.percent, button: "syncing" };
  const incomplete = others.find((i) => i.state === "incomplete" || i.state === "unread");
  if (incomplete?.state === "unread") return { text: t(incomplete.name === "platform" ? "shell.status.platformUnread" : "shell.status.shieldedUnread"), percent: null, button: "incomplete" };
  if (incomplete) return { text: t(incomplete.name === "platform" ? "shell.status.platformIncomplete" : "shell.status.shieldedIncomplete"), percent: null, button: "incomplete" };
  return { text: syncState(sync), percent: null, button: "synced" };
}

/** A Sync progress row's text. */
export function itemText(i: SyncItem, percent: (p: number) => string): string {
  switch (i.state) {
    case "done":
      return t("shell.sync.done");
    case "syncing":
      return i.percent !== null ? t("shell.sync.items.scan", { percent: percent(i.percent) }) : t("shell.sync.items.syncing");
    case "incomplete":
      return t(i.name === "platform" && !i.pass ? "shell.sync.items.platformIncomplete" : "shell.sync.items.shieldedIncomplete");
    case "unread":
      return t("shell.sync.items.unread");
    case "unlock":
      return t("shell.sync.items.unlock");
    case "unlockFully":
      return t("shell.sync.items.unlockFully");
    case "watchOnly":
      return t("shell.sync.items.watchOnly");
    case "offline":
      return t("shell.status.notSyncing");
    case "waiting":
      return t("shell.sync.waiting");
  }
}
