import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Backend } from "./backend";
import * as cmd from "./generated/commands";
import type { AppEvent } from "./generated/commands";
import type { JsonValue } from "./generated/serde_json/JsonValue";

/** True inside the Tauri webview. */
export const inTauri = () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export class TauriBackend implements Backend {
  readonly kind = "tauri";
  readonly locale = undefined;

  launchInfo = () => cmd.launchInfo();
  openNetwork = () => cmd.openNetwork(null, null);
  listWallets = () => cmd.listWallets();
  lockStatus = () => cmd.lockStatus();
  unlock = (passphrase: string) => cmd.unlock(passphrase);
  // The lock report (`flows`) is for the lease views (U-07); the shell needs the status.
  lock = () => cmd.lock().then((outcome) => outcome.status);
  balance = (walletId: string) => cmd.balance(walletId);
  syncStatus = () => cmd.syncStatus();
  historyPage = cmd.historyPage;
  txDetail = cmd.txDetail;
  onEvent = (listener: (e: AppEvent) => void) => listen<AppEvent>(cmd.ENGINE_EVENT, (e) => listener(e.payload));
  now = () => new Date();
}

/** Measurement and self-test commands; not part of `Backend`. */
export const tauriTools = {
  startupMark: cmd.startupMark,
  selftestReport: (report: unknown) => cmd.selftestReport(report as JsonValue),
  selftestCheckpoint: cmd.selftestCheckpoint,
  ipcBenchRows: cmd.ipcBenchRows,
  historyPageTimed: cmd.historyPageTimed,
  probeUnlisted: cmd.probeUnlisted,
  /** A plugin command that is not installed (U6: the renderer cannot reach a shell). */
  invokeUnknownPlugin: () => invoke<unknown>("plugin:shell|execute", { program: "sh", args: ["-c", "id"] }),
};
