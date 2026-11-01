// Commands the 0.1 release lanes add to dw-app that the renderer is built against before they land:
// lane E2's network choice and Platform addresses (contract m4-dashpay-engine v40 §2.12 and §8, branch
// `dw/rel-platform-addr`), and lane E1's shielded balance (a placeholder until its contract). Each is
// called by name through `callPending`, which answers `null` while dw-app has no such command (Tauri's
// ACL refusal or "Command … not found"; the fixture's `platform.not_implemented`), so a screen shows
// its "unavailable" state. The types below are the generated bindings' shapes by hand (the codegen
// test refuses files in `generated/` that dw-app does not write). When a command is generated into
// `generated/commands.ts`, its adapter line in `IpcBackend` (api/tauri.ts) becomes `cmd.<name>`, its
// type becomes an import from `generated/`, and its entry leaves `PENDING_COMMANDS`.

import { invoke } from "@tauri-apps/api/core";
import type { GrantRequest } from "./generated/GrantRequest";
import type { NetworkInfo } from "./generated/NetworkInfo";

/** The commands' wire names, one place for the adapter and the fixture. */
export const PENDING_COMMANDS = {
  getNetworkChoice: "get_network_choice",
  setNetworkChoice: "set_network_choice",
  platformAddresses: "platform_addresses",
  platformActivity: "platform_activity",
  platformMoveToCoreQuote: "platform_move_to_core_quote",
  platformMoveToCore: "platform_move_to_core",
  shieldedBalance: "shielded_balance",
} as const;

/** `ChosenNetwork`: the networks the user chooses between (developer networks are launch flags). */
export type NetworkChoice = "mainnet" | "testnet";

/** `NetworkChoiceView`: the network choice of onboarding and Settings (contract §8). */
export interface NetworkChoiceView {
  /** The saved choice; mainnet when none was saved. */
  chosen: NetworkChoice;
  /** A choice was saved (`false`: first run). */
  saved: boolean;
  /** The network this run opened. */
  launched: NetworkInfo;
  /** `--network` named it: the saved choice applies only to a launch without it. */
  forcedByFlag: boolean;
  /** The choice takes effect at the next start. */
  restartNeeded: boolean;
}

/** `PlatformAddressBalance`: the wallet's Platform addresses as the last sync pass left them (§2.12). */
export interface PlatformAddressBalance {
  credits: string;
  /** `credits` in whole duffs: what Home adds to the spendable total. */
  duffs: number;
  /** The funded addresses (bech32m), the largest first. */
  addresses: { address: string; credits: string }[];
  /** Unix seconds of this session's last sync pass; `null` before one. */
  syncedAt: number | null;
  syncing: boolean;
}

export type PlatformActivityKind = "incoming" | "outgoing" | "moveToCore";
export type PlatformActivityState = "done" | "pending" | "failed";

/** `PlatformActivity`: one Platform-address activity row, for Activity. */
export interface PlatformActivity {
  id: string;
  kind: PlatformActivityKind;
  state: PlatformActivityState;
  credits: string;
  address: string | null;
  to: string | null;
  expectedDuffs: number | null;
  /** Unix seconds. */
  at: number;
}

/** `MoveToCoreQuote`: what "Move to Dash wallet" takes and pays out. */
export interface MoveToCoreQuote {
  credits: string;
  feeCredits: string;
  expectedDuffs: number;
  /** The wallet's own Core receive address. */
  to: string;
  grant: GrantRequest;
}

/** `MoveToCoreOutcome`: `done`, or `pending` when Platform's answer said nothing either way. */
export interface MoveToCoreOutcome {
  id: string;
  credits: string;
  expectedDuffs: number;
  to: string;
  state: PlatformActivityState;
}

/** Whether `e` says the command does not exist in this dw-app. dw-app's ACL manifest lists every
 * command it has, so Tauri answers an unknown one from the ACL check: "Command x not allowed by
 * ACL" in a release build, "x not allowed. Command not found" in a debug build; "Command x not
 * found" is the handler lookup's. A command that exists but has no capability is absent the same. */
export function commandAbsent(e: unknown): boolean {
  if (typeof e === "string") return /^command .* (not found|not allowed by acl)/i.test(e) || /not allowed\. command not found/i.test(e);
  return typeof e === "object" && e !== null && "code" in e && (e as { code: unknown }).code === "platform.not_implemented";
}

/** `command`'s answer in `value` (a command that returns nothing answers `null`); `null` itself
 * when dw-app has no such command. Any other failure is thrown. */
export async function callPending<T>(command: string, args?: Record<string, unknown>): Promise<{ value: T } | null> {
  try {
    return { value: await invoke<T>(command, args) };
  } catch (e) {
    if (commandAbsent(e)) return null;
    throw e;
  }
}

/** A command that must exist for the call to mean anything (a save, a move of funds): its absence
 * is an error the screen shows. */
export async function callRequired<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const got = await callPending<T>(command, args);
  if (got === null) throw new Error("This is not available in this build.");
  return got.value;
}
