// The in-app network choice (0.1 release plan, "Network"): mainnet (the default) or testnet,
// at onboarding and in Settings ▸ General. It is what the next start opens; the window keeps the
// network it was started on, so a different choice asks for a restart.

import type { NetworkKind } from "../api/generated/NetworkKind";
import type { NetworkChoice } from "../api/pending";

export const NETWORK_CHOICES: { choice: NetworkChoice; label: string }[] = [
  { choice: "mainnet", label: "Mainnet" },
  { choice: "testnet", label: "Testnet" },
];

export const NetworkText = {
  title: "Network",
  hint: "Mainnet is the real Dash network. Testnet uses test coins with no value.",
  unavailable: "Choosing the network is not available in this build yet.",
  notSaved: "The network choice could not be saved.",
  /** A window started with `--network regtest` or `devnet-<name>` is not on either choice. */
  pinned: (current: string) => `This window was started with --network ${current}, which wins until the next start without it.`,
  restart: (choice: NetworkChoice) => `Restart Dash Wallet to use ${choice === "mainnet" ? "Mainnet" : "Testnet"}.`,
};

/** Whether the saved `choice` is not the network `current` window runs on, so a restart applies
 * it. A window on a devnet or regtest is not on either choice: it says so instead (`pinned`). */
export function restartNeeded(choice: NetworkChoice | null | undefined, current: NetworkKind): boolean {
  return (current === "mainnet" || current === "testnet") && !!choice && choice !== current;
}

export const pinned = (current: NetworkKind): boolean => current !== "mainnet" && current !== "testnet";
