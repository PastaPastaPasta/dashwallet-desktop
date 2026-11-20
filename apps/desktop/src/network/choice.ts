// The in-app network choice (0.1 release plan, "Network"): mainnet (the default) or testnet, at
// onboarding and in Settings ▸ General. Choosing moves the window to that network at once
// (`switch_network`, row 74) and saves the choice for the next start; a window started with
// `--network` (or a flag of one network) stays where it is and only saves it (`forcedByFlag`).
// This file holds the words.

import type { ChosenNetwork } from "../api/generated/ChosenNetwork";
import { t } from "../i18n";

export const NETWORK_CHOICES: { choice: ChosenNetwork; readonly label: string }[] = (["mainnet", "testnet"] as const).map((choice) => ({
  choice,
  get label() {
    return networkName(choice);
  },
}));

export const networkName = (choice: string) =>
  choice === "mainnet" || choice === "testnet" || choice === "regtest" || choice === "devnet" ? t(`network.${choice}`) : choice[0]?.toUpperCase() + choice.slice(1);

/** "a", "a and b", "a, b and c". */
const listed = (items: string[]) =>
  items.length < 2 ? items.join("") : t("common.listLast", { rest: items.slice(0, -1).join(t("common.listSeparator")), last: items.at(-1)! });

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
  get notSwitched() {
    return t("network.notSwitched");
  },
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
   * proof or a grant, whose leases a lock ends. */
  busy: (waits: string[], mixing: boolean, lockStops: string[] = []) => {
    const lower = (items: string[]) => listed(items.map((w) => w[0].toLocaleLowerCase(t.language.tag) + w.slice(1)));
    const wait = waits.length > 0 ? t("network.waitFor", { flows: lower(waits) }) : "";
    const text = !wait ? t("network.stopMixing") : mixing ? t("network.waitOrStopMixing", { wait }) : t("network.waitBeforeSwitching", { wait });
    return lockStops.length > 0 ? `${text} ${t("network.lockStops", { flows: lower(lockStops) })}` : text;
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
  /** After a switch that failed: the window went back to the network it left. */
  backOn: (to: ChosenNetwork, from: ChosenNetwork) => t("network.backOn", { to: networkName(to), from: networkName(from) }),
  /** After a switch that failed, and the way back too: the splash's Retry opens `from`. */
  noneOpen: (to: ChosenNetwork, from: ChosenNetwork) => t("network.noneOpen", { to: networkName(to), from: networkName(from) }),
  /** After a switch: links that arrived while it ran were read for the network the window left
   * (`from`, a directory name), so they were not opened. */
  linksDropped: (from: string) => t("network.linksDropped", { network: from === "mainnet" || from === "testnet" ? networkName(from) : from }),
};
