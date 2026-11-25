// Every sync the user waits on, as the status bar and Sync progress show them: the Dash network
// (SPV), Platform (the DashPay bring-up and the Platform addresses' first pass) and Shielded (the
// account's first scan). The bar says "Synced" only when none is syncing or unfinished; a sync
// that waits for an unlock, or that a watch-only wallet cannot run, is not one the bar waits on,
// and Sync progress says why.

import type { DashPaySyncStatus } from "../api/generated/DashPaySyncStatus";
import type { ShieldedStatus } from "../api/generated/ShieldedStatus";
import type { StartupStatus } from "../api/generated/StartupStatus";
import type { SyncStatus } from "../api/generated/SyncStatus";
import { shieldedReceiveGate, shieldedScanPercent } from "../shielded/syncing";
import { isSyncing, syncPercent, syncState } from "./sync";
import { t } from "../i18n";

export type SyncItemName = "core" | "platform" | "shielded";

/** `done`; `syncing` (with a percent when known); `incomplete`: it stopped short and tries again
 * later; `unlock`: it waits for a (full) unlock; `watchOnly`: this wallet cannot run it; `offline`
 * and `waiting`: the network's own (not running, not read yet). */
export type SyncItemState = "done" | "syncing" | "incomplete" | "unlock" | "watchOnly" | "offline" | "waiting";

export interface SyncItem {
  name: SyncItemName;
  state: SyncItemState;
  percent: number | null;
}

const item = (name: SyncItemName, state: SyncItemState, percent: number | null = null): SyncItem => ({ name, state, percent });

export function coreItem(sync: SyncStatus | undefined): SyncItem {
  if (!sync) return item("core", "waiting");
  if (!sync.running) return item("core", "offline");
  return isSyncing(sync) ? item("core", "syncing") : item("core", "done");
}

/** The bring-up outcomes that stopped short: they try again at the next start (DASHPAY §3.2). */
const PLATFORM_INCOMPLETE: ReadonlySet<StartupStatus> = new Set(["discoveryFailed", "identityScanIncomplete", "partialNoIdentity", "partialAccountsPending", "seedBindingUnverified"]);

/** `null`: no reading (the command failed, or Platform is not on this network), so not waited on. */
export function platformItem(s: DashPaySyncStatus | null | undefined): SyncItem | null {
  if (!s) return null;
  if (s.startup === "starting") return item("platform", "syncing");
  const addresses = s.loops.find((l) => l.syncLoop === "platformAddressSync");
  if (addresses?.running && addresses.lastRunAt === null) return item("platform", "syncing");
  if (PLATFORM_INCOMPLETE.has(s.startup)) return item("platform", "incomplete");
  if (s.startup === "identityUnsettled") return item("platform", "unlock");
  return item("platform", "done");
}

/** `null` while Shielded is not on this network or not read. Not established yet is syncing, a
 * failed first pass too (passes run again); once established, a failed last pass is incomplete. */
export function shieldedItem(s: ShieldedStatus | null | undefined): SyncItem | null {
  if (!s?.active) return null;
  const gate = shieldedReceiveGate(s);
  if (gate === "unlock" || gate === "watchOnly") return item("shielded", gate);
  if (!s.ready) return item("shielded", "syncing", shieldedScanPercent(s));
  return s.lastPassFailed ? item("shielded", "incomplete") : item("shielded", "done");
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
  const incomplete = others.find((i) => i.state === "incomplete");
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
      return t(i.name === "platform" ? "shell.sync.items.platformIncomplete" : "shell.sync.items.shieldedIncomplete");
    case "unlock":
      return t("shell.sync.items.unlock");
    case "watchOnly":
      return t("shell.sync.items.watchOnly");
    case "offline":
      return t("shell.status.notSyncing");
    case "waiting":
      return t("shell.sync.waiting");
  }
}
