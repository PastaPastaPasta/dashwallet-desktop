// The window shell (UX-SPEC §4.1, DASHPAY §4 R11): toolbar, sidebar and status bar.

import { useCallback, useId, useRef, useState, type ReactNode } from "react";
import type { DashPayStatus } from "../api/generated/DashPayStatus";
import type { IdentitySummary } from "../api/generated/IdentitySummary";
import type { LockStatus } from "../api/generated/LockStatus";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import type { SyncStatus } from "../api/generated/SyncStatus";
import type { WalletSummary } from "../api/generated/WalletSummary";
import { useBackend } from "../api/queries";
import { navigate, switchWallet, ui, useUi, type Route } from "../app/ui";
import { namesState, type NamesState } from "../dashpay/status";
import { useT, type Translate } from "../i18n";
import { ExperimentalBadge } from "./basics";
import { barSync, type SyncItem } from "../format/syncItems";
import { Avatar } from "../screens/contacts/Avatar";

interface SidebarItem {
  route: Route;
  /** 18 pt outline glyph in currentColor; macOS uses SF Symbols here (UX-SPEC §2.7). */
  icon: ReactNode;
}

const HOME: SidebarItem = { route: "home", icon: <path d="M3 9.5 10 3.5l7 6V17a1 1 0 0 1-1 1h-4v-5H8v5H4a1 1 0 0 1-1-1V9.5Z" /> };
const PAY: SidebarItem = { route: "pay", icon: <path d="M5 15 15 5M8 5h7v7" /> };
const RECEIVE: SidebarItem = { route: "receive", icon: <path d="M15 5 5 15m7 0H5V8" /> };
const CONTACTS_ICON = (
  <>
    <circle cx="10" cy="7" r="3" />
    <path d="M4 17c.6-3 3-4.5 6-4.5s5.4 1.5 6 4.5" />
  </>
);
const ACTIVITY: SidebarItem = { route: "activity", icon: <path d="M3 5h2m3 0h9M3 10h2m3 0h9M3 15h2m3 0h9" /> };
const COINJOIN: SidebarItem = {
  route: "coinJoin",
  icon: (
    <>
      <circle cx="7.5" cy="10" r="4.5" />
      <circle cx="12.5" cy="10" r="4.5" />
    </>
  ),
};

/** Masternodes, last, when the wallet has something to do with them (MG-00 D1, MG-09 D3). */
const MASTERNODES: SidebarItem = {
  route: "masternodes",
  icon: (
    <>
      <rect x="3" y="3.5" width="14" height="5" rx="1" />
      <rect x="3" y="11.5" width="14" height="5" rx="1" />
      <path d="M6 6h.01M6 14h.01" />
    </>
  ),
};

/** Settings, last and apart: pinned to the foot of the sidebar (DEC-1058 #7). */
const SETTINGS: SidebarItem = {
  route: "settings",
  icon: (
    <>
      <circle cx="10" cy="10" r="2.5" />
      <circle cx="10" cy="10" r="5.5" />
      <path d="M10 2v2.5M10 15.5V18M2 10h2.5M15.5 10H18M4.3 4.3l1.8 1.8M13.9 13.9l1.8 1.8M4.3 15.7l1.8-1.8M13.9 6.1l1.8-1.8" />
    </>
  ),
};

const EXPLORE: SidebarItem = { route: "explore", icon: <path d="M10 3a7 7 0 1 0 0 14 7 7 0 0 0 0-14Zm3 4-2 4-4 2 2-4 4-2Z" /> };

/** The sidebar's Contacts entry is Join DashPay until the wallet has an identity; a contested
 * username still has its identity, so it has Contacts (D11). */
export function sidebarJoins(status: DashPayStatus | undefined): boolean {
  return status?.kind === "noIdentity" || status?.kind === "registering";
}

/** Home, Pay, Receive, Contacts (Join DashPay while there is no identity; a badge for requests
 * waiting), Activity, CoinJoin and Explore when enabled (DASHPAY §4). */
const badgeText = (n: number) => (n > 99 ? "99+" : String(n));

export function Sidebar({ joined, requests, masternodes, explore }: { joined: boolean; requests: number; masternodes: { shown: boolean; badge: number }; explore: boolean }) {
  const t = useT();
  const route = useUi((s) => s.route);
  const contacts: SidebarItem = joined ? { route: "contacts", icon: CONTACTS_ICON } : { route: "joinDashPay", icon: CONTACTS_ICON };
  const items = [HOME, PAY, RECEIVE, contacts, ACTIVITY, COINJOIN, ...(explore ? [EXPLORE] : []), ...(masternodes.shown ? [MASTERNODES] : [])];
  const badges: Partial<Record<Route, number>> = { contacts: requests, masternodes: masternodes.badge };
  const item = (item: SidebarItem) => {
    const current = item.route === route;
    const count = badges[item.route] ?? 0;
    const badge = count > 0 ? count : null;
    return (
      <li key={item.route}>
        <button
          type="button"
          className={`sidebar-item ${current ? "is-current" : ""}`}
          data-testid={`sidebar-${item.route}`}
          aria-current={current ? "page" : undefined}
          onClick={() => navigate(item.route)}
        >
          <svg className="sidebar-icon" viewBox="0 0 20 20" aria-hidden="true">
            {item.icon}
          </svg>
          <span className="sidebar-title">
            {t(`routes.${item.route}`)}
            {item.route === "coinJoin" && " "}
            {item.route === "coinJoin" && <ExperimentalBadge testId="sidebar-coinJoin-experimental" />}
          </span>
          {badge !== null && (
            <span className="sidebar-badge" data-testid={`sidebar-${item.route}-badge`} aria-label={t("shell.sidebar.waiting", { count: badge })}>
              {badgeText(badge)}
            </span>
          )}
        </button>
      </li>
    );
  };
  return (
    <nav className="sidebar" aria-label={t("shell.sidebar.label")} data-testid="sidebar">
      <ul role="list" className="sidebar-list">
        {items.map(item)}
      </ul>
      <ul role="list" className="sidebar-list sidebar-foot">
        {item(SETTINGS)}
      </ul>
    </nav>
  );
}

function EyeIcon({ hidden }: { hidden: boolean }) {
  return (
    <svg viewBox="0 0 20 20" aria-hidden="true" className="toolbar-icon">
      <path d="M2 10s3-5.5 8-5.5S18 10 18 10s-3 5.5-8 5.5S2 10 2 10Z" />
      <circle cx="10" cy="10" r="2.5" />
      {hidden && <path d="M3 17 17 3" />}
    </svg>
  );
}

function LockIcon() {
  return (
    <svg viewBox="0 0 20 20" aria-hidden="true" className="toolbar-icon">
      <rect x="4.5" y="9" width="11" height="8" rx="1.5" />
      <path d="M7 9V6.5a3 3 0 0 1 6 0V9" />
    </svg>
  );
}

function BellIcon() {
  return (
    <svg viewBox="0 0 20 20" aria-hidden="true" className="toolbar-icon">
      <path d="M5 14V9a5 5 0 0 1 10 0v5l1.5 1.5h-13L5 14Z" />
      <path d="M8.5 17.5h3" />
    </svg>
  );
}

/** What the identity chip shows (DASHPAY §4): a registration's progress, or the main identity,
 * which opens My Profile. `null` hides it: with no identity (Home's card and the sidebar lead to
 * Join DashPay; a watch-only wallet cannot join, DP1-08 D3), while DashPay starts or is
 * unavailable, or after a status read that failed (D24).
 * `names`: what the main identity's names say (DP1-08 D23). */
export type ChipState =
  /** `label`: the registration's username once its row is read (DP1-08 S11). */
  | { kind: "registering"; label: string | null }
  | { kind: "contest"; label: string }
  | { kind: "identity"; identity: string; username: string | null; names: NamesState }
  | null;

export function chipState(
  status: DashPayStatus | undefined,
  main: IdentitySummary | undefined,
  registration?: RegistrationStatus,
): ChipState {
  switch (status?.kind) {
    case "registering":
      return { kind: "registering", label: registration?.draft === status.draft ? registration.label : null };
    case "contestPending":
      return { kind: "contest", label: status.label };
    case "ready":
      return { kind: "identity", identity: status.main, username: main?.mainName ?? null, names: namesState(main) };
    default:
      return null;
  }
}

function chipLabel(t: Translate, name: string | null, names: NamesState): string {
  if (name) return `@${name}`;
  switch (names) {
    case "unknown":
      return t("shell.toolbar.usernameNotLoaded");
    case "none":
      return t("shell.toolbar.noUsername");
    default:
      return t("shell.toolbar.updating");
  }
}

function chipText(t: Translate, chip: Exclude<ChipState, { kind: "identity" } | null>): string {
  switch (chip.kind) {
    case "contest":
      return t("shell.toolbar.contestPending", { name: `@${chip.label}` });
    case "registering":
      return chip.label ? t("shell.toolbar.registeringName", { name: `@${chip.label}` }) : t("shell.toolbar.registering");
  }
}

function IdentityChip({ chip }: { chip: ChipState }) {
  const t = useT();
  if (chip === null) return null;
  if (chip.kind !== "identity") {
    return (
      // A contested name's identity has My Profile (DP1-10 D21); a registration shows its progress.
      <button type="button" className="identity-chip" data-testid="toolbar-chip" onClick={() => navigate(chip.kind === "contest" ? "profile" : "joinDashPay")}>
        {chipText(t, chip)}
      </button>
    );
  }
  const name = chip.username;
  return (
    <button
      type="button"
      className="identity-chip"
      data-testid="toolbar-chip"
      aria-label={name ? t("shell.toolbar.myProfileNamed", { name: `@${name}` }) : t("shell.toolbar.myProfile")}
      onClick={() => navigate("profile")}
    >
      <Avatar letters={(name ?? "?").slice(0, 1).toUpperCase()} size="chip" identity={chip.identity} own />
      <bdi>{chipLabel(t, name, chip.names)}</bdi>
    </button>
  );
}

function WalletPicker({ wallets, current }: { wallets: WalletSummary[]; current: WalletSummary | undefined }) {
  const t = useT();
  if (wallets.length < 2) return <bdi className="toolbar-wallet">{current?.name}</bdi>;
  return (
    <select
      className="toolbar-wallet wallet-picker"
      data-testid="toolbar-wallet-picker"
      aria-label={t("shell.toolbar.wallet")}
      value={current?.walletId}
      onChange={(e) => switchWallet(e.target.value)}
    >
      {wallets.map((w) => (
        <option key={w.walletId} value={w.walletId}>
          {w.name}
        </option>
      ))}
    </select>
  );
}

export function Toolbar({
  wallets,
  wallet,
  chip,
  unread,
  canLock,
  onLock,
  bellPanel,
  bellOwner,
}: {
  wallets: WalletSummary[];
  wallet: WalletSummary | undefined;
  chip: ChipState;
  /** Unread DashPay events (the bell's badge); `null` while unknown or without an identity. */
  unread: number | null;
  canLock: boolean;
  onLock: () => void;
  /** The bell's popover (DP2-09 D11), given its id and `close`; without it the bell opens the
   * Notifications page. */
  bellPanel?: (id: string, close: (o?: { refocus?: boolean }) => void) => ReactNode;
  /** Whose bell `bellPanel` is (wallet and identity): another owner closes it (DP2-09 D27). */
  bellOwner?: string;
}) {
  const t = useT();
  const discreet = useUi((s) => s.discreet);
  const bellLabel = unread ? t("shell.toolbar.notificationsUnread", { count: unread }) : t("shell.toolbar.notifications");
  const discreetLabel = discreet ? t("shell.toolbar.showBalance") : t("shell.toolbar.hideBalance");
  // The owner whose bell is open: a switch of wallet (or main identity) while it is open, with the
  // keyboard, closes it rather than opening, and marking read, the next owner's (D27).
  const [openFor, setOpenFor] = useState<string | null>(null);
  if (openFor !== null && openFor !== bellOwner) setOpenFor(null);
  const bell = useRef<HTMLButtonElement>(null);
  const panelId = useId();
  const closeBell = useCallback(({ refocus = true }: { refocus?: boolean } = {}) => {
    setOpenFor(null);
    if (refocus) bell.current?.focus();
  }, []);
  const open = openFor !== null && openFor === bellOwner && !!bellPanel;
  return (
    <header className="toolbar" data-testid="toolbar">
      <span className="toolbar-start">
        <IdentityChip chip={chip} />
        <WalletPicker wallets={wallets} current={wallet} />
      </span>
      <span className="toolbar-actions">
        <span className="toolbar-bell" data-bell>
          <button
            type="button"
            className="toolbar-button"
            data-testid="toolbar-bell"
            aria-label={bellLabel}
            title={bellLabel}
            ref={bell}
            aria-haspopup={bellPanel ? "dialog" : undefined}
            aria-expanded={bellPanel ? open : undefined}
            aria-controls={open ? panelId : undefined}
            onClick={() => (bellPanel ? setOpenFor(open ? null : (bellOwner ?? null)) : navigate("notifications"))}
          >
            <BellIcon />
            {!!unread && (
              <span className="toolbar-badge" data-testid="toolbar-bell-badge" aria-hidden="true">
                {badgeText(unread)}
              </span>
            )}
          </button>
          {open && bellPanel(panelId, closeBell)}
        </span>
        <button
          type="button"
          className="toolbar-button"
          data-testid="toolbar-discreet"
          aria-pressed={discreet}
          aria-label={discreetLabel}
          title={discreetLabel}
          onClick={() => ui.set({ discreet: !discreet })}
        >
          <EyeIcon hidden={discreet} />
        </button>
        <button
          type="button"
          className="toolbar-button"
          data-testid="toolbar-lock"
          aria-label={t("shell.toolbar.lockWallet")}
          title={t("shell.toolbar.lockWallet")}
          disabled={!canLock}
          onClick={onLock}
        >
          <LockIcon />
        </button>
      </span>
    </header>
  );
}

export function StatusBar({
  sync,
  items = [],
  lock,
  unitName,
  hd,
  onUnlock,
  onSync,
  governance = null,
}: {
  sync: SyncStatus | undefined;
  /** Platform's and Shielded's syncs for the open wallet (`useSyncItems`): the bar says Synced only
   * once they are done too. */
  items?: readonly (SyncItem | null)[];
  lock: LockStatus | undefined;
  unitName: string;
  hd: boolean;
  /** Opens the unlock sheet: the lock state is a button while unlocked for mixing only (U-02 D12). */
  onUnlock: () => void;
  /** Opens Sync progress. */
  onSync?: () => void;
  /** Masternodes' voting item (MG-09 D7), before the sync state as in MG-00's frame. */
  governance?: ReactNode;
}) {
  const t = useT();
  const { locale } = useBackend();
  const bar = barSync(sync, items);
  const buttonText = bar.button && t(`shell.status.${bar.button}`);
  const lockText = lock && t(`shell.status.lock.${lock.state}`);
  return (
    <footer className="status-bar" data-testid="status-bar">
      <span className="status-left">
        <span role="status" data-testid="status-state">
          {bar.text}
        </span>
        {bar.percent !== null && (
          <>
            <span aria-hidden="true">{new Intl.NumberFormat(locale, { style: "percent", minimumFractionDigits: 1, maximumFractionDigits: 1 }).format(bar.percent / 100)}</span>
            <progress className="status-progress" max={100} value={bar.percent} aria-label={t("shell.status.syncProgress")} />
          </>
        )}
      </span>
      <span className="status-right">
        <span>{unitName}</span>
        {hd && <span>{t("shell.status.hd")}</span>}
        {lock?.state === "unlockedMixingOnly" ? (
          <button type="button" className="status-button" data-testid="status-unlock" title={t("shell.status.unlockFully")} onClick={onUnlock}>
            {lockText}
          </button>
        ) : (
          lock && <span>{lockText}</span>
        )}
        {sync && <span>{t("shell.status.peers", { count: sync.connectedPeers })}</span>}
        {governance}
        {buttonText &&
          (onSync ? (
            <button type="button" className="status-button" data-testid="status-sync" title={t("shell.status.showSync")} onClick={onSync}>
              {buttonText}
            </button>
          ) : (
            <span>{buttonText}</span>
          ))}
      </span>
    </footer>
  );
}
