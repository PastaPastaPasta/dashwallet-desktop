import { createContext, useContext, useEffect, useRef, useState } from "react";
import { errorMessage, type Backend } from "../api/backend";
import type { LockStatus } from "../api/generated/LockStatus";
import type { LaunchInfo } from "../api/generated/LaunchInfo";
import type { TxRow } from "../api/generated/TxRow";
import { useBackend, useDashPayShell, useLockStatus, useOpenNetwork, useSyncStatus, useWallets } from "../api/queries";
import { DashButton, DashLogo } from "../components/basics";
import { EmptyState, LoadingState } from "../components/history";
import { SheetLayer, useLayeredSheet } from "../components/sheet";
import { chipState, Sidebar, StatusBar, Toolbar } from "../components/shell";
import { showToast, ToastHost } from "../components/toast";
import { ForgotPassphraseSheet } from "../screens/ForgotPassphrase";
import { LockScreen, UnlockSheet } from "../screens/LockScreen";
import { Onboarding } from "../screens/Onboarding";
import { Overview } from "../screens/Overview";
import { ReceivePage } from "../screens/Receive";
import { RecoveryPhraseSheet } from "../screens/RecoveryPhrase";
import { ContactsPage, PayPage, Placeholder, ToolsPage } from "../screens/Routes";
import { SettingsPage } from "../screens/settings/SettingsPage";
import { AboutSheet, OpenUriSheet } from "../screens/ShellDialogs";
import { Transactions } from "../screens/Transactions";
import { TxDetailSheet } from "../screens/TxDetailSheet";
import { ACTIVITY_EVENTS, armedFor, AutoLockTimer } from "../settings/autoLock";
import { useDisplay } from "../format/display";
import { SecurityText } from "../settings/security";
import { useShellBridge } from "./shellBridge";
import { mark } from "./startup";
import { useThemeAttribute } from "./theme";
import { navigate, ui, useUi } from "./ui";

const closeDialog = () => ui.set({ dialog: null, recoveryWallet: null });
const closeOnboarding = () => ui.set({ onboarding: false });
const closeForgot = () => ui.set({ forgotPassphrase: false });

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

/** Auto lock (U-04 D12): armed while an encrypted vault holds its key and a wait is chosen;
 * input in the window restarts it, and on expiry the vault locks as the Lock button locks it. */
function useAutoLock(backend: Backend, status: LockStatus | undefined, secs: number | null) {
  const armed = secs !== null && armedFor(status?.state, status?.encrypted);
  useEffect(() => {
    if (!armed || secs === null) return;
    const timer = new AutoLockTimer();
    timer.arm(secs, () =>
      backend.lock().then(
        () => showToast(SecurityText.lockedAutomatically),
        (e) => showToast(`Could not lock: ${errorMessage(e)}`),
      ),
    );
    const onActivity = () => timer.activity();
    for (const e of ACTIVITY_EVENTS) window.addEventListener(e, onActivity, { capture: true, passive: true });
    return () => {
      timer.disarm();
      for (const e of ACTIVITY_EVENTS) window.removeEventListener(e, onActivity, { capture: true });
    };
  }, [backend, armed, secs]);
}

function Shell() {
  const backend = useBackend();
  const launch = useLaunch();
  const { network } = launch;
  const unitName = useDisplay(network).name;
  const route = useUi((s) => s.route);
  const detail = useUi((s) => s.detail);
  const dialog = useUi((s) => s.dialog);
  const recoveryWallet = useUi((s) => s.recoveryWallet);
  const walletId = useUi((s) => s.walletId);
  const discreet = useUi((s) => s.discreet);
  const addWallet = useUi((s) => s.onboarding);
  const wiping = useUi((s) => s.wiping);
  const forgot = useUi((s) => s.forgotPassphrase);
  const secretShown = useUi((s) => s.secrets > 0);
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
  useAutoLock(backend, lock.data, useUi((s) => s.prefs.autoLockSecs));
  // The native menus' Discreet Mode check and Lock Wallet follow the window, and so does its
  // content protection while a flow that shows or takes a recovery phrase is open (U-02 D15).
  // One call at a time, in order: a stale `secretShown` never lands last (review r1 #2). The
  // shell answers once the protection is in effect, and only then do those flows show their
  // secret part (`protection: "on"`, D27); only the latest call's answer counts.
  const protectionTries = useUi((s) => s.protectionTries);
  const shellSynced = useRef(Promise.resolve());
  const syncs = useRef(0);
  useEffect(() => {
    const sync = { discreet, canLock, secretShown };
    const call = ++syncs.current;
    if (!secretShown) ui.set({ protection: "off" });
    else if (ui.get().protection !== "on") ui.set({ protection: "pending" });
    const settle = (protection: "on" | "failed") => {
      if (secretShown && call === syncs.current) ui.set({ protection });
    };
    shellSynced.current = shellSynced.current.then(() =>
      backend.shellSync(sync).then(
        () => settle("on"),
        () => settle("failed"),
      ),
    );
  }, [backend, discreet, canLock, secretShown, protectionTries]);

  // Onboarding (U-02 D1): a network without a wallet opens it, and it stays open until its
  // Done. The lock screen replaces it, and drops what it held: Add Wallet… closes, the first
  // wallet's flow starts again at Welcome once unlocked.
  const noWallet = wallets.isSuccess && wallets.data.length === 0;
  // A wipe in progress holds it back until the vault is gone or its failure was seen (U-04 D33).
  const onboarding = !locked && !wiping && (addWallet || noWallet);
  useEffect(() => {
    if (noWallet) ui.set({ onboarding: true });
    else if (locked && addWallet) ui.set({ onboarding: false });
  }, [noWallet, locked, addWallet]);
  // The new wallet is listed before onboarding closes: a list still empty would open it again
  // (review r1 #5).
  const finishOnboarding = async (id: string | null) => {
    await wallets.refetch();
    ui.set({ onboarding: false, ...(id ? { walletId: id } : {}) });
    navigate("home");
  };

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
    // Wallet pages start over for another wallet: no address, filter or form state of the last one
    // survives a switch (U-03 D24). Pay keys its form inside (Routes.tsx).
    else if (route === "home") content = <Overview key={wallet.walletId} walletId={wallet.walletId} network={network} onOpen={open} />;
    else if (route === "pay") content = <PayPage walletId={wallet.walletId} network={network} />;
    else if (route === "receive") content = <ReceivePage key={wallet.walletId} walletId={wallet.walletId} network={network} />;
    else content = <Transactions key={wallet.walletId} walletId={wallet.walletId} network={network} onOpen={open} />;
  } else if (route === "contacts") content = <ContactsPage />;
  else if (route === "tools") content = <ToolsPage />;
  else if (route === "settings") content = <SettingsPage />;
  else content = <Placeholder route={route} />;

  // A contested username still has its identity: Contacts, not Join DashPay (D11).
  const joined = !(dashpay.status?.kind === "noIdentity" || dashpay.status?.kind === "registering");
  const sheet = !!(detail && wallet && !locked);
  const layered = useLayeredSheet();
  const [layer, setLayer] = useState<HTMLDivElement | null>(null);
  // A dialog asked for while locked, onboarding, or behind a sheet is dropped, not kept for later.
  const shown = locked || onboarding || sheet || layered ? null : dialog;
  useEffect(() => {
    if (dialog !== shown) closeDialog();
  }, [dialog, shown]);
  return (
    <>
      {/* Behind a sheet or the lock screen the shell takes no focus and is hidden from assistive tech. */}
      <div className="app-chrome" inert={locked || onboarding || forgot || sheet || layered || shown !== null}>
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
        <StatusBar
          sync={sync.data}
          lock={lock.data}
          unitName={unitName}
          hd={wallet?.hd ?? false}
          onUnlock={() => ui.set({ dialog: "unlock" })}
        />
        </SheetLayer.Provider>
      </div>
      {/* Sheets opened inside the shell (Pay, Receive): above it, below onboarding and the lock
          screen, which cover it (U-02 D31). */}
      <div className="sheet-layer" ref={setLayer} inert={locked || onboarding} />
      {sheet && <TxDetailSheet key={`${wallet!.walletId}:${detail}`} walletId={wallet!.walletId} txid={detail!} network={network} />}
      {shown === "openUri" && <OpenUriSheet network={network.dirName} />}
      {shown === "about" && <AboutSheet launch={launch} />}
      {shown === "recoveryPhrase" && <RecoveryPhraseSheet walletId={recoveryWallet ?? wallet?.walletId ?? null} onClose={closeDialog} />}
      {shown === "unlock" && <UnlockSheet onClose={closeDialog} />}
      {onboarding && <Onboarding network={network} onCancel={noWallet ? null : closeOnboarding} onDone={finishOnboarding} />}
      {locked && <LockScreen network={network} covered={forgot} onForgot={() => ui.set({ forgotPassphrase: true })} />}
      {/* Over the lock screen, and still open once the recovery unlocked the vault (U-02 D11). */}
      {forgot && !onboarding && <ForgotPassphraseSheet onClose={closeForgot} />}
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
