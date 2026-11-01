// Commands the 0.1 release lanes add to dw-app (lane E2's network choice; lanes E1 and E2's
// shielded and Platform balances) that the renderer is built against before they land. Each one
// is called by name through `callPending`, which answers `null` while dw-app has no such command
// (Tauri: "Command … not found"; the fixture: `platform.not_implemented`), so a screen shows its
// "unavailable" state. When a command is generated into `generated/commands.ts`, the adapter
// line in `IpcBackend` (api/tauri.ts) becomes `cmd.<name>` and this file loses its entry.

import { invoke } from "@tauri-apps/api/core";

/** The commands' names, one place for the adapter and the fixture. */
export const PENDING_COMMANDS = {
  getNetworkChoice: "get_network_choice",
  setNetworkChoice: "set_network_choice",
  shieldedBalance: "shielded_balance",
  platformBalance: "platform_address_balance",
} as const;

/** The network a start opens when no `--network` is given (mainnet by default). */
export type NetworkChoice = "mainnet" | "testnet";

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
