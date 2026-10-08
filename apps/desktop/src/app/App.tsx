import { createContext, useContext, useEffect } from "react";
import { errorMessage } from "../api/backend";
import type { LaunchInfo } from "../api/generated/LaunchInfo";
import type { TxRow } from "../api/generated/TxRow";
import { useBackend, useLockStatus, useOpenNetwork, useSyncStatus, useWallets } from "../api/queries";
import { DashButton, DashLogo } from "../components/basics";
import { EmptyState, LoadingState } from "../components/history";
import { Sidebar, StatusBar, Toolbar } from "../components/shell";
import { showToast, ToastHost } from "../components/toast";
import { LockScreen } from "../screens/LockScreen";
import { Overview } from "../screens/Overview";
import { Transactions } from "../screens/Transactions";
import { TxDetailSheet } from "../screens/TxDetailSheet";
import { mark } from "./startup";
import { useThemeAttribute } from "./theme";
import { ui, useUi } from "./ui";

export const LaunchContext = createContext<LaunchInfo | null>(null);
export const useLaunch = () => useContext(LaunchContext)!;

/** Splash (UX-SPEC §4.2) while the engine opens the network, or its error with a retry. */
function Splash({ error, onRetry }: { error: string | null; onRetry: () => void }) {
  return (
    <div className="splash" data-testid="splash">
      <DashLogo className="splash-logo" />
      {error ? (
        <>
          <p className="splash-error" role="alert">
            Dash Wallet could not open the wallet: {error}
          </p>
          <DashButton variant="plainBlue" onClick={onRetry}>
            Try again
          </DashButton>
        </>
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
  const { network } = useLaunch();
  const page = useUi((s) => s.page);
  const detail = useUi((s) => s.detail);
  const lock = useLockStatus();
  const sync = useSyncStatus();
  const wallets = useWallets();
  const wallet = wallets.data?.[0];
  const locked = lock.data?.state === "locked";
  const open = (row: TxRow) => ui.set({ detail: row.txid });

  let content;
  if (wallets.isPending) content = <LoadingState testId="wallets-loading" text="Loading wallet…" />;
  else if (wallets.isError)
    content = (
      <main className="page" data-testid="page-wallets-error">
        <div className="page-column">
          <EmptyState testId="wallets-error" title="Could not load the wallets" message={errorMessage(wallets.error)} />
        </div>
      </main>
    );
  else if (!wallet)
    content = (
      <main className="page" data-testid="page-no-wallet">
        <div className="page-column">
          <EmptyState testId="no-wallet" title="No wallet on this network yet" />
        </div>
      </main>
    );
  else if (page === "overview") content = <Overview walletId={wallet.walletId} network={network} onOpen={open} />;
  else content = <Transactions walletId={wallet.walletId} network={network} onOpen={open} />;

  const canLock = lock.data?.encrypted === true && !locked;
  const lockNow = () => backend.lock().catch((e) => showToast(`Could not lock: ${errorMessage(e)}`));
  const sheet = !!(detail && wallet && !locked);
  return (
    <>
      {/* Behind a sheet or the lock screen the shell takes no focus and is hidden from assistive tech. */}
      <div className="app-chrome" inert={locked || sheet}>
        <Toolbar walletName={wallet?.name ?? null} canLock={canLock} onLock={() => void lockNow()} />
        <div className="app-body">
          <Sidebar />
          {content}
        </div>
        <StatusBar sync={sync.data} lock={lock.data} unitName={network.unitName} hd={wallet?.hd ?? false} />
      </div>
      {sheet && <TxDetailSheet walletId={wallet!.walletId} txid={detail!} network={network} />}
      {locked && <LockScreen network={network} />}
    </>
  );
}

export function App() {
  useThemeAttribute();
  // Opening the network loads the wallet database; the splash is up meanwhile.
  const network = useOpenNetwork();
  useEffect(() => mark("shell"), []);
  return (
    <div className="app" data-testid="app-shell">
      {network.isSuccess ? (
        <Shell />
      ) : (
        <Splash error={network.isError ? errorMessage(network.error) : null} onRetry={() => void network.refetch()} />
      )}
      <ToastHost />
    </div>
  );
}
