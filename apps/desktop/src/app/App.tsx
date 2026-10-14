import { createContext, useContext, useEffect, useState } from "react";
import { errorMessage } from "../api/backend";
import type { LaunchInfo } from "../api/generated/LaunchInfo";
import type { TxRow } from "../api/generated/TxRow";
import { useBackend, useDashPayShell, useLockStatus, useOpenNetwork, useSyncStatus, useWallets } from "../api/queries";
import { DashButton, DashLogo } from "../components/basics";
import { EmptyState, LoadingState } from "../components/history";
import { SheetLayer, useLayeredSheet } from "../components/sheet";
import { chipState, Sidebar, StatusBar, Toolbar } from "../components/shell";
import { showToast, ToastHost } from "../components/toast";
import { LockScreen } from "../screens/LockScreen";
import { Overview } from "../screens/Overview";
import { ReceivePage } from "../screens/Receive";
import { ContactsPage, PayPage, Placeholder, ToolsPage } from "../screens/Routes";
import { AboutSheet, OpenUriSheet } from "../screens/ShellDialogs";
import { Transactions } from "../screens/Transactions";
import { TxDetailSheet } from "../screens/TxDetailSheet";
import { useShellBridge } from "./shellBridge";
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
  const launch = useLaunch();
  const { network } = launch;
  const route = useUi((s) => s.route);
  const detail = useUi((s) => s.detail);
  const dialog = useUi((s) => s.dialog);
  const walletId = useUi((s) => s.walletId);
  const discreet = useUi((s) => s.discreet);
  const lock = useLockStatus();
  const sync = useSyncStatus();
  const wallets = useWallets();
  const wallet = wallets.data?.find((w) => w.walletId === walletId) ?? wallets.data?.[0];
  const dashpay = useDashPayShell(wallet?.walletId);
  const locked = lock.data?.state === "locked";
  const open = (row: TxRow) => ui.set({ detail: row.txid });

  const canLock = lock.data?.encrypted === true && !locked;
  const lockNow = () => {
    if (canLock) backend.lock().catch((e) => showToast(`Could not lock: ${errorMessage(e)}`));
  };
  useShellBridge(backend, network.dirName, launch.selftestLinkTrace, lockNow);
  // The native menus' Discreet Mode check and Lock Wallet follow the window.
  useEffect(() => {
    backend.shellSync({ discreet, canLock }).catch(() => {});
  }, [backend, discreet, canLock]);

  let content;
  if (route === "home" || route === "activity" || route === "pay" || route === "receive") {
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
    else if (route === "home") content = <Overview walletId={wallet.walletId} network={network} onOpen={open} />;
    else if (route === "pay") content = <PayPage walletId={wallet.walletId} network={network} />;
    else if (route === "receive") content = <ReceivePage walletId={wallet.walletId} network={network} />;
    else content = <Transactions walletId={wallet.walletId} network={network} onOpen={open} />;
  } else if (route === "contacts") content = <ContactsPage />;
  else if (route === "tools") content = <ToolsPage />;
  else content = <Placeholder route={route} />;

  // A contested username still has its identity: Contacts, not Join DashPay (D11).
  const joined = !(dashpay.status?.kind === "noIdentity" || dashpay.status?.kind === "registering");
  const sheet = !!(detail && wallet && !locked);
  const layered = useLayeredSheet();
  const [layer, setLayer] = useState<HTMLDivElement | null>(null);
  // A dialog asked for while locked or behind a sheet is dropped, not kept for later.
  const shown = locked || sheet || layered ? null : dialog;
  useEffect(() => {
    if (dialog !== shown) ui.set({ dialog: null });
  }, [dialog, shown]);
  return (
    <>
      {/* Behind a sheet or the lock screen the shell takes no focus and is hidden from assistive tech. */}
      <div className="app-chrome" inert={locked || sheet || layered || shown !== null}>
        <SheetLayer.Provider value={layer}>
        <Toolbar
          wallets={wallets.data ?? []}
          wallet={wallet}
          chip={chipState(dashpay.status, dashpay.mainName)}
          unread={dashpay.unread}
          canLock={canLock}
          onLock={lockNow}
        />
        <div className="app-body">
          <Sidebar joined={joined} requests={dashpay.requests} />
          {content}
        </div>
        <StatusBar sync={sync.data} lock={lock.data} unitName={network.unitName} hd={wallet?.hd ?? false} />
        </SheetLayer.Provider>
      </div>
      {/* Sheets opened inside the shell (Pay, Receive): above it, below the lock screen. */}
      <div className="sheet-layer" ref={setLayer} inert={locked} />
      {sheet && <TxDetailSheet walletId={wallet!.walletId} txid={detail!} network={network} />}
      {shown === "openUri" && <OpenUriSheet network={network.dirName} />}
      {shown === "about" && <AboutSheet launch={launch} />}
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
