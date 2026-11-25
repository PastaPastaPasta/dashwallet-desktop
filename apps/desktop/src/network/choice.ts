// The in-app network choice (0.1 release plan, "Network"): mainnet (the default) or testnet, at
// onboarding and in Settings ▸ General. Choosing moves the window to that network at once
// (`switch_network`, row 74) and saves the choice for the next start; a window started with
// `--network` (or a flag of one network) stays where it is and only saves it (`forcedByFlag`).
// This file holds the words.

import type { ChosenNetwork } from "../api/generated/ChosenNetwork";
import type { FlowKindName } from "../api/generated/FlowKindName";
import { t } from "../i18n";

export const NETWORK_CHOICES: { choice: ChosenNetwork; readonly label: string }[] = (["mainnet", "testnet"] as const).map((choice) => ({
  choice,
  get label() {
    return networkName(choice);
  },
}));

export const networkName = (choice: string) =>
  choice === "mainnet" || choice === "testnet" || choice === "regtest" || choice === "devnet" ? t(`network.${choice}`) : choice[0]?.toUpperCase() + choice.slice(1);

/** "a", "a and b", "a, b and c": the catalog's own words in English and the pseudo-locales, the
 * language's list pattern in a translation. */
export const listed = (items: string[]) => {
  const { tag, pseudo } = t.language;
  if (tag !== "en" && !pseudo) return new Intl.ListFormat(tag, { type: "conjunction" }).format(items);
  return items.length < 2 ? items.join("") : t("common.listLast", { rest: items.slice(0, -1).join(t("common.listSeparator")), last: items.at(-1)! });
};

/** What a refused switch waits for: the payment being sent, or a flow. */
export type BusyItem = "payment" | FlowKindName;

export const NetworkText = {
  get title() {
    return t("network.title");
  },
  get hint() {
    return t("network.hint");
  },
  get unavailable() {
    return t("network.unavailable");
  },
  /** A switch refused for a reason the renderer has no words of its own for: the engine's message. */
  notSwitchedBecause: (message: string, code?: string) =>
    code === undefined ? t("network.notSwitchedBecause", { message }) : t("network.notSwitchedCode", { message, code }),
  /** After a switch that went through but whose choice was not written: the next start opens the old network. */
  get notSaved() {
    return t("network.notSaved");
  },
  /** A window started with `--network regtest` or `devnet-<name>` is not on either choice. */
  pinned: (current: string, next: ChosenNetwork | null) =>
    next
      ? t("network.pinnedNext", { current: networkName(current), next: networkName(next) })
      : t("network.pinnedSame", { current: networkName(current) }),
  /** The blocking card while the switch runs. */
  opening: (choice: ChosenNetwork) => t("network.opening", { network: networkName(choice) }),
  confirmTitle: (choice: ChosenNetwork) => t("network.confirmTitle", { network: networkName(choice) }),
  confirmText: (to: ChosenNetwork, from: ChosenNetwork) => t("network.confirmText", { to: networkName(to), from: networkName(from) }),
  confirmPending: (from: ChosenNetwork) => t("network.confirmPending", { from: networkName(from) }),
  get confirm() {
    return t("network.confirm");
  },
  /** Before Switch is enabled: the pending-transfer reads have not answered yet. */
  get checking() {
    return t("network.checking");
  },
  /** Refused: closing the network would cut what runs on it. `lockStops`: flows waiting for a
   * proof or a grant, whose leases a lock ends; each of `waits`. */
  busy: (waits: BusyItem[], mixing: boolean, lockStops: FlowKindName[] = []) => {
    const named = (items: BusyItem[]) => listed(items.map((w) => t(`network.busyItem.${w}`)));
    if (waits.length === 0) return t("network.stopMixing");
    const flows = named(waits);
    if (lockStops.length === 0) return mixing ? t("network.waitOrStopMixing", { flows }) : t("network.waitBeforeSwitching", { flows });
    const stops = named(lockStops);
    return mixing ? t("network.waitOrStopMixingLockStops", { flows, stops }) : t("network.waitBeforeSwitchingLockStops", { flows, stops });
  },
  /** Refused: the network's work in progress did not finish in time. */
  get busyDraining() {
    return t("network.busyDraining");
  },
  get walletOpening() {
    return t("network.walletOpening");
  },
  get shieldedUnresolved() {
    return t("network.shieldedUnresolved");
  },
  get transferPreparing() {
    return t("transfer.preparingBlocksClose");
  },
  /** After a switch that failed: the window went back to the network it left. */
  backOn: (to: ChosenNetwork, from: ChosenNetwork) => t("network.backOn", { to: networkName(to), from: networkName(from) }),
  /** After a switch that failed, and the way back too: the splash's Retry opens `from`. */
  noneOpen: (to: ChosenNetwork, from: ChosenNetwork) => t("network.noneOpen", { to: networkName(to), from: networkName(from) }),
  /** After a switch: links that arrived while it ran were read for the network the window left
   * (`from`, a directory name), so they were not opened. */
  linksDropped: (from: string) => t("network.linksDropped", { network: from === "mainnet" || from === "testnet" ? networkName(from) : from }),
};
