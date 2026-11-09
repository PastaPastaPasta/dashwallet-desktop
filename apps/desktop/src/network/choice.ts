// The in-app network choice (0.1 release plan, "Network"): mainnet (the default) or testnet, at
// onboarding and in Settings ▸ General. Choosing moves the window to that network at once
// (`switch_network`, row 74) and saves the choice for the next start; a window started with
// `--network` (or a flag of one network) stays where it is and only saves it (`forcedByFlag`).
// This file holds the words.

import type { ChosenNetwork } from "../api/generated/ChosenNetwork";

export const NETWORK_CHOICES: { choice: ChosenNetwork; label: string }[] = [
  { choice: "mainnet", label: "Mainnet" },
  { choice: "testnet", label: "Testnet" },
];

const networkName = (choice: ChosenNetwork) => (choice === "mainnet" ? "Mainnet" : "Testnet");

/** "a", "a and b", "a, b and c". */
const listed = (items: string[]) => (items.length < 2 ? items.join("") : `${items.slice(0, -1).join(", ")} and ${items.at(-1)}`);

export const NetworkText = {
  title: "Network",
  hint: "Mainnet is the real Dash network. Testnet uses test coins with no value.",
  unavailable: "Choosing the network is not available in this build yet.",
  notSwitched: "Dash Wallet could not switch networks.",
  /** After a switch that went through but whose choice was not written: the next start opens the old network. */
  notSaved: "The network choice could not be saved.",
  /** A window started with `--network regtest` or `devnet-<name>` is not on either choice. */
  pinned: (current: string) => `This window was started with --network ${current}, which wins until the next start without it.`,
  /** The blocking card while the switch runs. */
  opening: (choice: ChosenNetwork) => `Opening ${networkName(choice)}…`,
  confirmTitle: (choice: ChosenNetwork) => `Switch to ${networkName(choice)}?`,
  confirmText: (to: ChosenNetwork, from: ChosenNetwork) =>
    `Dash Wallet locks this wallet and opens ${networkName(to)}. Payments you haven't sent are dropped. Your ${networkName(from)} wallet stays on this computer.`,
  confirmPending: (from: ChosenNetwork) =>
    `Unfinished transfers and locks stay on ${networkName(from)}. You can finish them after switching back.`,
  confirm: "Switch",
  /** Refused: closing the network would cut what runs on it. */
  busy: (waits: string[], mixing: boolean) => {
    const wait = waits.length > 0 ? `Wait for ${listed(waits.map((w) => w[0].toLowerCase() + w.slice(1)))} to finish` : "";
    if (wait && mixing) return `${wait}, or stop mixing, before switching networks.`;
    return `${wait || "Stop mixing"} before switching networks.`;
  },
  walletOpening: "A wallet is still opening. Switch networks once it is open.",
  shieldedUnresolved: "A shielded payment is still being confirmed. Switch networks once it is.",
  /** After a switch that failed: the window went back to the network it left. */
  backOn: (to: ChosenNetwork, from: ChosenNetwork) => `${networkName(to)} did not open. Dash Wallet is back on ${networkName(from)}.`,
  /** After a switch that failed, and the way back too: the splash's Retry opens `from`. */
  noneOpen: (to: ChosenNetwork, from: ChosenNetwork) => `${networkName(to)} did not open, and neither did ${networkName(from)} again.`,
};
