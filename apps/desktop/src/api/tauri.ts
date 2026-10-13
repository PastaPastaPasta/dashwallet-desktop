import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Backend } from "./backend";
import * as cmd from "./generated/commands";
import type { AppEvent } from "./generated/commands";
import type { JsonValue } from "./generated/serde_json/JsonValue";

// Read once, when the module loads: the fixture backend's mock IPC installs its own
// `__TAURI_INTERNALS__` later, and must not make Tauri-only tools look reachable.
const IN_TAURI = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** True inside the Tauri webview (not under the fixture backend's mock IPC). */
export const inTauri = () => IN_TAURI;

/** The one `Backend`: dw-app's commands over IPC. In a fixture build outside Tauri the IPC is
 * the fixture backend's mock (src/fixture), so both modes run this same code. */
export class IpcBackend implements Backend {
  constructor(
    readonly kind: Backend["kind"],
    /** The fixture's fixed clock; the system's otherwise. */
    private readonly clock: () => Date = () => new Date(),
    readonly locale: string | undefined = undefined,
  ) {}

  launchInfo = () => cmd.launchInfo();
  openNetwork = () => cmd.openNetwork(null, null);
  listWallets = () => cmd.listWallets();
  lockStatus = () => cmd.lockStatus();
  unlock = (passphrase: string) => cmd.unlock(passphrase);
  lock = () => cmd.lock();
  lastLockReport = () => cmd.lastLockReport();
  leases = () => cmd.leases();
  dispatchStatus = cmd.dispatchStatus;
  balance = (walletId: string) => cmd.balance(walletId);
  syncStatus = () => cmd.syncStatus();
  historyPage = cmd.historyPage;
  txDetail = cmd.txDetail;
  onEvent = (listener: (e: AppEvent) => void) => listen<AppEvent>(cmd.ENGINE_EVENT, (e) => listener(e.payload));
  now = () => this.clock();
}

/** Measurement and self-test commands; not part of `Backend`. */
export const tauriTools = {
  startupMark: cmd.startupMark,
  selftestReport: (report: unknown) => cmd.selftestReport(report as JsonValue),
  selftestCheckpoint: cmd.selftestCheckpoint,
  selftestUnlock: () => cmd.selftestUnlock(),
  ipcBenchRows: cmd.ipcBenchRows,
  historyPageTimed: cmd.historyPageTimed,
  probeUnlisted: cmd.probeUnlisted,
  /** A plugin command that is not installed (U6: the renderer cannot reach a shell). */
  invokeUnknownPlugin: () => invoke<unknown>("plugin:shell|execute", { program: "sh", args: ["-c", "id"] }),
};
