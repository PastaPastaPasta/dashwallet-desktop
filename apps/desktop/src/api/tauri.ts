import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Backend } from "./backend";
import type { Balances } from "./generated/Balances";
import type { EngineSignal } from "./generated/EngineSignal";
import type { HistoryRequest } from "./generated/HistoryRequest";
import type { HistoryRows } from "./generated/HistoryRows";
import type { LaunchInfo } from "./generated/LaunchInfo";
import type { LockStatus } from "./generated/LockStatus";
import type { SyncStatus } from "./generated/SyncStatus";
import type { TxDetails } from "./generated/TxDetails";
import type { WalletSummary } from "./generated/WalletSummary";

/** Event name of `dw_app::bridge::SIGNAL_EVENT`. */
export const SIGNAL_EVENT = "engine://signal";

/** True inside the Tauri webview. */
export const inTauri = () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export class TauriBackend implements Backend {
  readonly kind = "tauri";
  readonly locale = undefined;

  launchInfo = () => invoke<LaunchInfo>("launch_info");
  openNetwork = () => invoke<LockStatus>("open_network");
  listWallets = () => invoke<WalletSummary[]>("list_wallets");
  lockStatus = () => invoke<LockStatus>("lock_status");
  unlock = (passphrase: string) => invoke<LockStatus>("unlock", { passphrase });
  lock = () => invoke<LockStatus>("lock");
  balance = (walletId: string) => invoke<Balances | null>("balance", { walletId });
  syncStatus = () => invoke<SyncStatus>("sync_status");
  historyPage = (walletId: string, request: HistoryRequest) => invoke<HistoryRows>("history_page", { walletId, request });
  txDetail = (walletId: string, txid: string) => invoke<TxDetails>("tx_detail", { walletId, txid });
  onSignal = (listener: (s: EngineSignal) => void) => listen<EngineSignal>(SIGNAL_EVENT, (e) => listener(e.payload));
  now = () => new Date();
}

/** Measurement and self-test commands; not part of `Backend`. */
export const tauriTools = {
  startupMark: (mark: string) => invoke<number>("startup_mark", { mark }),
  selftestReport: (report: unknown) => invoke<void>("selftest_report", { report }),
  selftestCheckpoint: (screen: string) => invoke<void>("selftest_checkpoint", { screen }),
  ipcBenchRows: (rows: number) => invoke<HistoryRows>("ipc_bench_rows", { rows }),
  probeUnlisted: () => invoke<string>("probe_unlisted"),
  /** A plugin command that is not installed (U6: the renderer cannot reach a shell). */
  invokeUnknownPlugin: () => invoke<unknown>("plugin:shell|execute", { program: "sh", args: ["-c", "id"] }),
};
