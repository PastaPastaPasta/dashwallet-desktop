import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import { createContext, useContext, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { errorMessage, type Backend } from "../api/backend";
import type { LockStatus } from "../api/generated/LockStatus";
import type { LaunchInfo } from "../api/generated/LaunchInfo";
import type { TxRow } from "../api/generated/TxRow";
import type { WalletSummary } from "../api/generated/WalletSummary";
import { closeWalletGate, forgetWalletContent, lockGeneration, useWalletGateClosed } from "../api/lockGate";
import { useBackend, useDashPayShell, useHistoryGapReads, useLockStatus, useOpenNetwork, useSyncStatus, useWallets } from "../api/queries";
import { DashButton, DashLogo } from "../components/basics";
import { EMPTY, walletListGate, walletReadyToOpen } from "../components/emptyStates";
import { EmptyState, LoadingState } from "../components/history";
import { SwitchingCard } from "../components/networkPicker";
import { SheetLayer, useLayeredSheet } from "../components/sheet";
import { chipState, Sidebar, sidebarJoins, StatusBar, Toolbar } from "../components/shell";
import { showShellToast, ToastHost } from "../components/toast";
import { takeSwitchNote } from "../network/switch";
import { ForgotPassphraseSheet } from "../screens/ForgotPassphrase";
import { LockScreen, UnlockSheet } from "../screens/LockScreen";
import { Onboarding } from "../screens/Onboarding";
import { Overview } from "../screens/Overview";
import { PreReleaseNotice } from "../screens/PreReleaseNotice";
import { ReceivePage } from "../screens/Receive";
import { RecoveryPhraseSheet } from "../screens/RecoveryPhrase";
import { MasternodeKeysSheet } from "../screens/masternodes/KeysSheet";
import { CoinJoinPage } from "../screens/coinjoin/CoinJoinPage";
import { MasternodesPage } from "../screens/masternodes/MasternodesPage";
import { GovernanceStatusItem } from "../screens/masternodes/StatusItem";
import { ContactsPage, PayPage, Placeholder } from "../screens/Routes";
import { AppConnectSheet } from "../screens/dashpay/AppConnectSheet";
import { JoinDashPayPage } from "../screens/dashpay/JoinDashPay";
import { MyProfilePage } from "../screens/dashpay/MyProfile";
import { NotificationsPage, NotificationsPopover } from "../screens/notifications/Notifications";
import { ReceivedToaster } from "../screens/notifications/Received";
import { unreadReceived, useReceived } from "../payments/received";
import { ToolsPage } from "../screens/tools/ToolsPage";
import { SettingsPage } from "../screens/settings/SettingsPage";
import { AboutSheet, OpenUriSheet } from "../screens/ShellDialogs";
import { SyncSheet } from "../screens/SyncSheet";
import { SavedLinksSheet } from "../screens/SavedLinks";
import { Transactions } from "../screens/Transactions";
import { TxDetailSheet } from "../screens/TxDetailSheet";
import { SourceDetailSheet } from "../screens/SourceDetailSheet";
import { rowSource } from "../history/sourceRows";
import { ACTIVITY_EVENTS, armedFor, AutoLockTimer } from "../settings/autoLock";
import { useDisplay } from "../format/display";
import { SecurityText } from "../settings/security";
import { useStatusRegistration } from "../dashpay/queries";
import { PictureScope, usePictureScopeValue } from "../dashpay/profileQueries";
import { useMasternodesItem } from "../governance/queries";
import { useSavedLinksOffer } from "./savedLinksOffer";
import { focusPage, useShellBridge } from "./shellBridge";
import { openFailure, type OpenFailure } from "./openFailure";
import { mark } from "./startup";
import { useThemeAttribute } from "./theme";
import { t, useLanguageAttributes, useT } from "../i18n";
import { savePrefs } from "../settings/prefs";
import { updatesModel, useAutomaticUpdateCheck } from "../settings/updates";
import { UpdateRestartNotice } from "../screens/settings/Updates";
import { useModel } from "../onboarding/model";
import { navigate, ui, useUi } from "./ui";

const closeDialog = () => ui.set({ dialog: null, recoveryWallet: null, mnKeysRole: null });
const closeOnboarding = () => ui.set({ onboarding: false });
const closeForgot = () => ui.set({ forgotPassphrase: false });
const closeConnect = () => {
  const [next = null, ...rest] = ui.get().appConnectQueue;
  ui.set({ appConnect: next, appConnectQueue: rest });
};

export const LaunchContext = createContext<LaunchInfo | null>(null);
export const useLaunch = () => useContext(LaunchContext)!;

/** Splash (UX-SPEC §4.2) while the engine opens the network, or its error, with a retry unless the file is from a newer
 * build (DP3-04b D35). */
function Splash({ error, onRetry }: { error: OpenFailure | null; onRetry: () => void }) {
  const tr = useT();
  return (
    <div className="splash" data-testid="splash">
      <DashLogo className="splash-logo" />
      {error ? (
        <>
          <p className="splash-error" role="alert">
            {error.text}
          </p>
          {error.retry && (
            <DashButton variant="plainBlue" onClick={onRetry}>
              {tr("app.tryAgain")}
            </DashButton>
          )}
        </>
      ) : (
        <>
          <span className="splash-bar" aria-hidden="true" />
          <p className="splash-text" role="status">
            {tr("app.loadingWallet")}
          </p>
        </>
      )}
    </div>
  );
}

/** Auto lock (U-04 D12): armed while an encrypted vault holds its key and a wait is chosen;
 * input in the window restarts it, and on expiry the vault locks as the Lock button locks it. */
function useAutoLock(backend: Backend, client: QueryClient, status: LockStatus | undefined, secs: number | null) {
  const armed = secs !== null && armedFor(status?.state, status?.encrypted);
  useEffect(() => {
    if (!armed || secs === null) return;
    const timer = new AutoLockTimer();
    timer.arm(secs, () => {
      const since = lockGeneration();
      return backend.lock().then(
        () => {
          closeWalletGate(client, since);
          showShellToast(SecurityText.lockedAutomatically);
        },
        (e) => showShellToast(t("app.lockFailed", { error: errorMessage(e) })),
      );
    });
    const onActivity = () => timer.activity();
    for (const e of ACTIVITY_EVENTS) window.addEventListener(e, onActivity, { capture: true, passive: true });
    return () => {
      timer.disarm();
      for (const e of ACTIVITY_EVENTS) window.removeEventListener(e, onActivity, { capture: true });
    };
  }, [backend, client, armed, secs]);
}

type WalletList = ReturnType<typeof useWallets>;

/** The window's wallet as the shell shows it: its pages, toolbar, sidebar, status bar and the
 * transaction sheet. Mounted only while the vault is not locked: the lock screen stands in
 * its place, and what it read goes with it (`forgetWalletContent`). */
function WalletChrome({
  wallets,
  wallet,
  walletsLoading,
  walletsError,
  sheet,
  inert,
  layerInert,
  canLock,
  onLock,
}: {
  wallets: WalletList;
  wallet: WalletSummary | undefined;
  walletsLoading: ReactNode;
  walletsError: ReactNode;
  sheet: boolean;
  inert: boolean;
  layerInert: boolean;
  canLock: boolean;
  onLock: () => void;
}) {
  const launch = useLaunch();
  const { network } = launch;
  const unitName = useDisplay(network).name;
  const route = useUi((s) => s.route);
  const detail = useUi((s) => s.detail);
  const lock = useLockStatus();
  const sync = useSyncStatus();
  useHistoryGapReads(sync.raw?.running === true, (wallets.data ?? []).map((w) => `${w.walletId}:${w.birthHeight}`).join(","));
  const dashpay = useDashPayShell(wallet?.walletId);
  const registration = useStatusRegistration(wallet?.walletId, dashpay.status);
  const open = (row: TxRow) => ui.set({ detail: row.txid });

  const masternodesItem = useMasternodesItem(wallet?.walletId ?? null);
  useEffect(() => {
    if (route === "masternodes" && masternodesItem.settled && !masternodesItem.shown) navigate("home");
  }, [route, masternodesItem.settled, masternodesItem.shown]);

  const walletsBlocked =
    walletsLoading ??
    (walletsError && (
      <main className="page" data-testid="page-wallets-error">
        <div className="page-column">{walletsError}</div>
      </main>
    ));
  let content;
  if (route === "home" || route === "activity" || route === "pay" || route === "receive" || route === "contacts" || route === "notifications" || route === "joinDashPay" || route === "profile" || route === "coinJoin" || route === "masternodes") {
    if (walletsBlocked) content = walletsBlocked;
    else if (!wallet)
      content = (
        <main className="page" data-testid="page-no-wallet">
          <div className="page-column">
            <EmptyState testId="no-wallet" art={EMPTY.wallet.art} title={EMPTY.wallet.title} message={EMPTY.wallet.message} />
          </div>
        </main>
      );
    // Wallet pages start over for another wallet: no address, filter or form state of the last one
    // survives a switch (U-03 D24). Pay keys its form inside (Routes.tsx).
    else if (route === "home") content = <Overview key={wallet.walletId} walletId={wallet.walletId} network={network} onOpen={open} />;
    else if (route === "pay") content = <PayPage walletId={wallet.walletId} network={network} />;
    else if (route === "receive") content = <ReceivePage key={wallet.walletId} walletId={wallet.walletId} network={network} />;
    else if (route === "contacts") content = <ContactsPage key={wallet.walletId} walletId={wallet.walletId} network={network} />;
    else if (route === "notifications") content = <NotificationsPage key={wallet.walletId} walletId={wallet.walletId} network={network} />;
    else if (route === "joinDashPay") content = <JoinDashPayPage key={wallet.walletId} walletId={wallet.walletId} network={network} />;
    else if (route === "profile") content = <MyProfilePage key={wallet.walletId} walletId={wallet.walletId} network={network} />;
    else if (route === "coinJoin") content = <CoinJoinPage key={wallet.walletId} walletId={wallet.walletId} network={network} />;
    // Hidden while Show Masternodes is Off, or Auto finds nothing (MG-00 D1, D2): the effect above
    // sends a link or a menu that opened it home. The page stays while Auto has no answer yet.
    else if (route === "masternodes" && (masternodesItem.shown || !masternodesItem.settled)) content = <MasternodesPage key={wallet.walletId} wallet={wallet} network={network} />;
    else if (route === "masternodes") content = <Overview key={wallet.walletId} walletId={wallet.walletId} network={network} onOpen={open} />;
    else content = <Transactions key={wallet.walletId} walletId={wallet.walletId} network={network} onOpen={open} />;
  } else if (route === "tools") content = <ToolsPage wallet={wallet ?? null} wallets={wallets.data ?? []} network={network} noWallet={walletsLoading ?? walletsError} />;
  else if (route === "settings") content = <SettingsPage network={network} />;
  else content = <Placeholder route={route} />;

  // The bell's popover once the main identity is known; before that the bell opens its page.
  const main = dashpay.main?.identity;
  // The bell's badge: DashPay events not seen, and payments that arrived and were not.
  const receivedUnread = unreadReceived(useReceived(wallet?.walletId));
  const unread = receivedUnread > 0 ? (dashpay.unread ?? 0) + receivedUnread : dashpay.unread;
  const bellOwner = wallet && main ? `${wallet.walletId}:${main}` : undefined;
  const bellPanel =
    wallet && main
      ? (id: string, close: (o?: { refocus?: boolean }) => void) => <NotificationsPopover key={bellOwner} walletId={wallet.walletId} identity={main} network={network} id={id} close={close} />
      : undefined;
  const joined = !sidebarJoins(dashpay.status);
  const [layer, setLayer] = useState<HTMLDivElement | null>(null);
  return (
    <>
      {/* Behind a sheet the shell takes no focus and is hidden from assistive tech. */}
      <div className="app-chrome" inert={inert}>
        <SheetLayer.Provider value={layer}>
          <Toolbar
            wallets={wallets.data ?? []}
            wallet={wallet}
            chip={chipState(dashpay.status, dashpay.main, registration)}
            unread={unread}
            canLock={canLock}
            onLock={onLock}
            bellPanel={bellPanel}
            bellOwner={bellOwner}
          />
          <div className="app-body">
            <Sidebar joined={joined} requests={dashpay.requests} masternodes={masternodesItem} />
            {content}
          </div>
          <StatusBar
            sync={sync.data}
            lock={lock.data}
            unitName={unitName}
            hd={wallet?.hd ?? false}
            onUnlock={() => ui.set({ dialog: "unlock" })}
            onSync={() => ui.set({ dialog: "sync" })}
            governance={wallet && masternodesItem.shown ? <GovernanceStatusItem key={wallet.walletId} walletId={wallet.walletId} /> : null}
          />
        </SheetLayer.Provider>
      </div>
      {/* Sheets opened inside the shell (Pay, Receive): above it, below onboarding, which covers it (U-02 D31). */}
      <div className="sheet-layer" ref={setLayer} inert={layerInert} />
      {sheet &&
        (rowSource({ txid: detail! }) ? (
          <SourceDetailSheet key={`${wallet!.walletId}:${detail}`} walletId={wallet!.walletId} txid={detail!} network={network} />
        ) : (
          <TxDetailSheet key={`${wallet!.walletId}:${detail}`} walletId={wallet!.walletId} txid={detail!} network={network} />
        ))}
    </>
  );
}

function Shell() {
  const tr = useT();
  const backend = useBackend();
  const client = useQueryClient();
  const launch = useLaunch();
  const { network } = launch;
  const detail = useUi((s) => s.detail);
  const dialog = useUi((s) => s.dialog);
  const recoveryWallet = useUi((s) => s.recoveryWallet);
  const mnKeysRole = useUi((s) => s.mnKeysRole);
  const walletId = useUi((s) => s.walletId);
  const discreet = useUi((s) => s.discreet);
  const addWallet = useUi((s) => s.onboarding);
  const wiping = useUi((s) => s.wiping);
  const forgot = useUi((s) => s.forgotPassphrase);
  const secretShown = useUi((s) => s.secrets > 0);
  // The first-run notice stays up until it is dismissed; the save that keeps it dismissed may
  // fail (the notice then shows again next start), so it does not wait for it.
  const noticeSaved = useUi((s) => s.prefs.preReleaseNoticeDismissed);
  const [noticeClosed, setNoticeClosed] = useState(false);
  const notice = !noticeSaved && !noticeClosed;
  const lock = useLockStatus();
  // Closed until a status says the vault is not locked, and from the lock's event until a status
  // read says so again (api/lockGate.ts). Before the first status there is nothing to show.
  const locked = useWalletGateClosed();
  const lockShown = lock.data?.state === "locked";
  // The gate dropped the wallet's queries as it closed; anything a view still mounted then asked
  // for goes too, now that the views are gone.
  useLayoutEffect(() => {
    if (locked) forgetWalletContent(client);
  }, [client, locked]);
  // Read once the vault is not locked; the shell holds no wallet until then.
  const wallets = useWallets(!locked);
  const wallet = locked ? undefined : (wallets.data?.find((w) => w.walletId === walletId) ?? wallets.data?.[0]);
  // Back from the lock screen, once the page is there again, focus lands on its title, as a link
  // or a menu leaves it.
  const backFromLock = useRef(false);
  useEffect(() => {
    if (locked && lockShown) backFromLock.current = true;
    else if (!locked && backFromLock.current && wallet) {
      backFromLock.current = false;
      if (document.activeElement === null || document.activeElement === document.body) focusPage();
    }
  }, [locked, lockShown, wallet]);

  const canLock = lock.data?.encrypted === true && !locked;
  const lockNow = () => {
    if (!canLock) return;
    // The answer closes the gate unless the lock state moved on before it came (its event, an unlock).
    const since = lockGeneration();
    backend.lock().then(
      () => closeWalletGate(client, since),
      (e) => showShellToast(t("app.lockFailed", { error: errorMessage(e) })),
    );
  };
  useShellBridge(backend, network.dirName, launch.selftestLinkTrace, lockNow);
  useAutoLock(backend, client, lock.data, useUi((s) => s.prefs.autoLockSecs));
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
  const gate = locked ? "loading" : walletListGate(wallets);
  const noWallet = gate === "empty";
  const walletsError = gate === "error" ? <EmptyState testId="wallets-error" title={tr("app.walletsFailed")} message={errorMessage(wallets.error)} /> : null;
  const walletsLoading = gate === "loading" ? <LoadingState testId="wallets-loading" text={tr("app.loadingWallet")} /> : null;
  // A wipe in progress holds it back until the vault is gone or its failure was seen (U-04 D33).
  const onboarding = !locked && !wiping && (addWallet || noWallet);
  useEffect(() => {
    if (noWallet) ui.set({ onboarding: true });
    else if (locked && addWallet) ui.set({ onboarding: false });
  }, [noWallet, locked, addWallet]);
  // The new wallet is listed before onboarding closes: a list still empty would open it again
  // (review r1 #5). A failed read that only still has the empty list from before the save stays
  // on the ready step; the screen reports it and tries the read again.
  const finishOnboarding = async (id: string | null) => {
    const result = await wallets.refetch();
    if (!walletReadyToOpen(id, result.data)) return;
    ui.set({ onboarding: false, ...(id ? { walletId: id } : {}) });
    navigate("home");
    savedLinks.trigger();
  };

  const sheet = !!(detail && wallet);
  const layered = useLayeredSheet();
  // A sign-in request waits while locked, during onboarding and behind the notice; it opens once
  // a wallet is open.
  const connect = useUi((s) => s.appConnect);
  const connectWallet = connect && wallet && !locked && !onboarding && !notice ? wallet : null;
  const savedLinks = useSavedLinksOffer(backend, {
    hasWallet: !!wallet,
    locked,
    covered: onboarding || wiping || forgot || sheet || layered || dialog !== null || connectWallet !== null,
  });
  const pictures = usePictureScopeValue(wallet?.walletId);
  // A dialog asked for while locked, onboarding, or behind a sheet is dropped, not kept for later.
  const shown = notice || locked || onboarding || sheet || layered || connectWallet ? null : dialog;
  useEffect(() => {
    if (dialog !== shown) closeDialog();
  }, [dialog, shown]);
  return (
    // Whose avatars every list may show (DP4-03 D12): the window's wallet, others' only with
    // "Load contact pictures" on.
    <PictureScope.Provider value={pictures}>
      <ReceivedToaster network={network} />
      {!locked && (
        <WalletChrome
          wallets={wallets}
          wallet={wallet}
          walletsLoading={walletsLoading}
          walletsError={walletsError}
          sheet={sheet}
          inert={notice || onboarding || forgot || sheet || layered || shown !== null || connectWallet !== null}
          layerInert={notice || onboarding}
          canLock={canLock}
          onLock={lockNow}
        />
      )}
      {connect && connectWallet && (
        <AppConnectSheet
          key={`${connectWallet.walletId}-${connect.linkId}`}
          walletId={connectWallet.walletId}
          linkId={connect.linkId}
          returnsTo={connect.returnsTo}
          network={network.dirName}
          onClose={closeConnect}
        />
      )}
      {shown === "openUri" && <OpenUriSheet network={network.dirName} />}
      {shown === "about" && <AboutSheet launch={launch} />}
      {shown === "recoveryPhrase" && <RecoveryPhraseSheet walletId={recoveryWallet ?? wallet?.walletId ?? null} onClose={closeDialog} />}
      {shown === "mnKeys" && wallet && <MasternodeKeysSheet key={wallet.walletId} walletId={wallet.walletId} watchOnly={wallet.watchOnly} role={mnKeysRole} onClose={closeDialog} />}
      {shown === "unlock" && <UnlockSheet onClose={closeDialog} />}
      {shown === "sync" && <SyncSheet onClose={closeDialog} />}
      {shown === "savedLinks" && <SavedLinksSheet onClose={closeDialog} />}
      {onboarding && <Onboarding network={network} inert={notice} onCancel={noWallet ? null : closeOnboarding} onDone={finishOnboarding} />}
      {/* In place of the wallet's views, not over them: a locked window holds nothing of the wallet. */}
      {locked && lockShown && <LockScreen network={network} covered={forgot || notice} onForgot={() => ui.set({ forgotPassphrase: true })} />}
      {/* Over the lock screen, and still open once the recovery unlocked the vault (U-02 D11). */}
      {forgot && !onboarding && !notice && <ForgotPassphraseSheet onClose={closeForgot} />}
      {/* Over everything, the lock screen too: the first thing a new install shows. */}
      {notice && (
        <PreReleaseNotice
          onDismiss={() => {
            setNoticeClosed(true);
            void savePrefs(backend, launch, { preReleaseNoticeDismissed: true }).catch(() => undefined);
          }}
        />
      )}
    </PictureScope.Provider>
  );
}

export function App() {
  useThemeAttribute();
  // The whole tree renders again in a new language: the screens' text and the formats that read
  // the locale (`Backend.locale`).
  useLanguageAttributes();
  // Opening the network loads the wallet database; the splash is up meanwhile.
  const network = useOpenNetwork();
  useEffect(() => mark("shell"), []);
  const backend = useBackend();
  useAutomaticUpdateCheck(backend);
  const restartRequired = useModel(updatesModel(backend)).view.kind === "restartRequired";
  // A network switch that failed booted this page again; it says what happened (row 74).
  useEffect(() => {
    const note = takeSwitchNote();
    if (note !== null) showShellToast(note, 8);
  }, []);
  // A network switch covers every page, onboarding and the lock screen until the page boots again
  // or the switch is refused; what is under its card takes no input (row 74).
  const switching = useUi((s) => s.switching) !== null;
  return (
    <div className="app" data-testid="app-shell">
      <div className="app-under-switch" inert={switching || restartRequired}>
        {network.isSuccess ? (
          <Shell />
        ) : (
          <Splash error={network.isError ? openFailure(network.error) : null} onRetry={() => void network.refetch()} />
        )}
      </div>
      <SwitchingCard />
      <UpdateRestartNotice />
      <ToastHost />
    </div>
  );
}
