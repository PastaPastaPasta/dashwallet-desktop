import { createContext, useContext, useEffect, type CSSProperties } from "react";
import { useQuery } from "@tanstack/react-query";
import type { LaunchInfo } from "../api/generated/LaunchInfo";
import type { TxRow } from "../api/generated/TxRow";
import { keys, useBackend, useLockStatus, useSyncStatus, useWallets } from "../api/queries";
import { LoadingState } from "../components/history";
import { iconFiles } from "../components/icons";
import { Sidebar, StatusBar, Toolbar } from "../components/shell";
import { ToastHost } from "../components/toast";
import { LockScreen } from "../screens/LockScreen";
import { Overview } from "../screens/Overview";
import { Transactions } from "../screens/Transactions";
import { TxDetailSheet } from "../screens/TxDetailSheet";
import { mark } from "./startup";
import { useAppearance } from "./theme";
import { ui, useUi } from "./ui";

export const LaunchContext = createContext<LaunchInfo | null>(null);
export const useLaunch = () => useContext(LaunchContext)!;

/** Splash (UX-SPEC §4.2) while the engine opens the network, or its error. */
function Splash({ error }: { error?: string }) {
  const logo = { "--logo-url": `url("${iconFiles.dashLogo.light}")` } as CSSProperties;
  return (
    <div className="splash" data-testid="splash">
      <span className="splash-logo" style={logo} role="img" aria-label="Dash" />
      {error ? (
        <p className="splash-error" role="alert">
          Dash Wallet could not open the wallet: {error}
        </p>
      ) : (
        <>
          <span className="splash-bar" aria-hidden="true" />
          <p className="splash-text" role="status">
            Loading wallet…
          </p>
        </>
      )}
    </div>
  );
}

function Shell() {
  const backend = useBackend();
  const launch = useLaunch();
  const page = useUi((s) => s.page);
  const detail = useUi((s) => s.detail);
  const lock = useLockStatus();
  const sync = useSyncStatus();
  const wallets = useWallets();
  const wallet = wallets.data?.[0];
  const locked = lock.data?.state === "locked";
  const network = launch.network;
  const open = (row: TxRow) => ui.set({ detail: row.txid });

  let content;
  if (wallets.isPending) content = <LoadingState testId="wallets-loading" text="Loading wallet…" />;
  else if (!wallet)
    content = (
      <main className="page" data-testid="page-no-wallet">
        <div className="page-column">
          <p className="empty-state-title">No wallet on this network yet.</p>
        </div>
      </main>
    );
  else if (page === "overview") content = <Overview walletId={wallet.walletId} network={network} onOpen={open} />;
  else content = <Transactions walletId={wallet.walletId} network={network} onOpen={open} />;

  const canLock = lock.data?.encrypted === true && lock.data.state !== "locked";
  return (
    <>
      <div className="app-chrome" inert={locked}>
        <Toolbar walletName={wallet?.name ?? null} canLock={canLock} onLock={() => void backend.lock()} />
        <div className="app-body">
          <Sidebar />
          {content}
        </div>
        <StatusBar sync={sync.data} lock={lock.data} unitName={network.unitName} />
      </div>
      {detail && wallet && !locked && <TxDetailSheet walletId={wallet.walletId} txid={detail} network={network} />}
      {locked && <LockScreen network={network} />}
    </>
  );
}

export function App() {
  const backend = useBackend();
  useAppearance();
  // Opening the network loads the wallet database; the splash is up meanwhile.
  const network = useQuery({ queryKey: keys.network, queryFn: () => backend.openNetwork() });
  useEffect(() => mark("shell"), []);
  return (
    <div className="app" data-testid="app-shell">
      {network.isSuccess ? <Shell /> : <Splash error={network.isError ? (network.error as { message?: string }).message ?? String(network.error) : undefined} />}
      <ToastHost />
    </div>
  );
}
