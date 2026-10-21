// The window shell (UX-SPEC §4.1, DASHPAY §4 R11): toolbar, sidebar and status bar.

import { useCallback, useId, useRef, useState, type ReactNode } from "react";
import type { DashPayStatus } from "../api/generated/DashPayStatus";
import type { IdentitySummary } from "../api/generated/IdentitySummary";
import type { LockStatus } from "../api/generated/LockStatus";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import type { SyncStatus } from "../api/generated/SyncStatus";
import type { WalletSummary } from "../api/generated/WalletSummary";
import { navigate, ui, useUi, type Route } from "../app/ui";
import { namesState, type NamesState } from "../dashpay/status";
import { isSyncing, syncPercent, syncState } from "../format/sync";

interface SidebarItem {
  route: Route;
  title: string;
  /** 18 pt outline glyph in currentColor; macOS uses SF Symbols here (UX-SPEC §2.7). */
  icon: ReactNode;
}

const HOME: SidebarItem = { route: "home", title: "Home", icon: <path d="M3 9.5 10 3.5l7 6V17a1 1 0 0 1-1 1h-4v-5H8v5H4a1 1 0 0 1-1-1V9.5Z" /> };
const PAY: SidebarItem = { route: "pay", title: "Pay", icon: <path d="M5 15 15 5M8 5h7v7" /> };
const RECEIVE: SidebarItem = { route: "receive", title: "Receive", icon: <path d="M15 5 5 15m7 0H5V8" /> };
const CONTACTS_ICON = (
  <>
    <circle cx="10" cy="7" r="3" />
    <path d="M4 17c.6-3 3-4.5 6-4.5s5.4 1.5 6 4.5" />
  </>
);
const ACTIVITY: SidebarItem = { route: "activity", title: "Activity", icon: <path d="M3 5h2m3 0h9M3 10h2m3 0h9M3 15h2m3 0h9" /> };
const COINJOIN: SidebarItem = {
  route: "coinJoin",
  title: "CoinJoin",
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
  title: "Masternodes",
  icon: (
    <>
      <rect x="3" y="3.5" width="14" height="5" rx="1" />
      <rect x="3" y="11.5" width="14" height="5" rx="1" />
      <path d="M6 6h.01M6 14h.01" />
    </>
  ),
};

/** Sections behind a feature that is off: Explore until its task. CoinJoin
 * shows, as in dash-qt, whose `-enablecoinjoin` is on by default (D-row "sidebar"). */
const EXPLORE_ENABLED = false;
const EXPLORE: SidebarItem = { route: "explore", title: "Explore", icon: <path d="M10 3a7 7 0 1 0 0 14 7 7 0 0 0 0-14Zm3 4-2 4-4 2 2-4 4-2Z" /> };

/** Home, Pay, Receive, Contacts (Join DashPay while there is no identity; a badge for requests
 * waiting), Activity, CoinJoin and Explore when enabled (DASHPAY §4). */
const badgeText = (n: number) => (n > 99 ? "99+" : String(n));

export function Sidebar({ joined, requests, masternodes }: { joined: boolean; requests: number; masternodes: { shown: boolean; badge: number } }) {
  const route = useUi((s) => s.route);
  const contacts: SidebarItem = joined
    ? { route: "contacts", title: "Contacts", icon: CONTACTS_ICON }
    : { route: "joinDashPay", title: "Join DashPay", icon: CONTACTS_ICON };
  const items = [HOME, PAY, RECEIVE, contacts, ACTIVITY, COINJOIN, ...(EXPLORE_ENABLED ? [EXPLORE] : []), ...(masternodes.shown ? [MASTERNODES] : [])];
  return (
    <nav className="sidebar" aria-label="Main" data-testid="sidebar">
      <ul role="list" className="sidebar-list">
        {items.map((item) => {
          const current = item.route === route;
          const count = item.route === "contacts" ? requests : item.route === "masternodes" ? masternodes.badge : 0;
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
                <span className="sidebar-title">{item.title}</span>
                {badge !== null && (
                  <span className="sidebar-badge" data-testid={`sidebar-${item.route}-badge`} aria-label={`${badge} waiting`}>
                    {badgeText(badge)}
                  </span>
                )}
              </button>
            </li>
          );
        })}
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

/** What the identity chip shows (DASHPAY §4): the way into DashPay, its progress, or the main
 * identity, which opens My Profile. `null` hides it (DashPay still starting, unavailable, or a
 * watch-only wallet, which cannot join: DP1-08 D3), as does a status read that failed (D24).
 * `names`: what the main identity's names say (DP1-08 D23). */
export type ChipState =
  | { kind: "join" }
  /** `label`: the registration's username once its row is read (DP1-08 S11). */
  | { kind: "registering"; label: string | null }
  | { kind: "contest"; label: string }
  | { kind: "identity"; username: string | null; names: NamesState }
  | null;

export function chipState(
  status: DashPayStatus | undefined,
  main: IdentitySummary | undefined,
  registration?: RegistrationStatus,
): ChipState {
  switch (status?.kind) {
    case "noIdentity":
      return status.reason === "watchOnly" ? null : { kind: "join" };
    case "registering":
      return { kind: "registering", label: registration?.draft === status.draft ? registration.label : null };
    case "contestPending":
      return { kind: "contest", label: status.label };
    case "ready":
      return { kind: "identity", username: main?.mainName ?? null, names: namesState(main) };
    default:
      return null;
  }
}

function chipLabel(name: string | null, names: NamesState): string {
  if (name) return `@${name}`;
  switch (names) {
    case "unknown":
      return "Username not loaded";
    case "none":
      return "No username";
    default:
      return "Updating…";
  }
}

function chipText(chip: Exclude<ChipState, { kind: "identity" } | null>): string {
  switch (chip.kind) {
    case "join":
      return "Join DashPay";
    case "contest":
      return `@${chip.label} · Pending`;
    case "registering":
      return chip.label ? `@${chip.label} · Registering` : "Registering…";
  }
}

function IdentityChip({ chip }: { chip: ChipState }) {
  if (chip === null) return null;
  if (chip.kind !== "identity") {
    return (
      // A contested name's identity has My Profile (DP1-10 D21); the others join or register.
      <button type="button" className="identity-chip" data-testid="toolbar-chip" onClick={() => navigate(chip.kind === "contest" ? "profile" : "joinDashPay")}>
        {chipText(chip)}
      </button>
    );
  }
  const name = chip.username;
  return (
    <button
      type="button"
      className="identity-chip"
      data-testid="toolbar-chip"
      aria-label={name ? `My profile, @${name}` : "My profile"}
      onClick={() => navigate("profile")}
    >
      <span className="identity-chip-avatar" aria-hidden="true">
        {(name ?? "?").slice(0, 1).toUpperCase()}
      </span>
      <span>{chipLabel(name, chip.names)}</span>
    </button>
  );
}

function WalletPicker({ wallets, current }: { wallets: WalletSummary[]; current: WalletSummary | undefined }) {
  if (wallets.length < 2) return <span className="toolbar-wallet">{current?.name}</span>;
  return (
    <select
      className="toolbar-wallet wallet-picker"
      data-testid="toolbar-wallet-picker"
      aria-label="Wallet"
      value={current?.walletId}
      onChange={(e) => ui.set({ walletId: e.target.value, detail: null })}
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
  bellPanel?: (id: string, close: () => void) => ReactNode;
  /** Whose bell `bellPanel` is (wallet and identity): another owner closes it (DP2-09 D27). */
  bellOwner?: string;
}) {
  const discreet = useUi((s) => s.discreet);
  const bellLabel = unread ? `Notifications, ${unread} unread` : "Notifications";
  // The owner whose bell is open: a switch of wallet (or main identity) while it is open, with the
  // keyboard, closes it rather than opening, and marking read, the next owner's (D27).
  const [openFor, setOpenFor] = useState<string | null>(null);
  if (openFor !== null && openFor !== bellOwner) setOpenFor(null);
  const bell = useRef<HTMLButtonElement>(null);
  const panelId = useId();
  const closeBell = useCallback(() => {
    setOpenFor(null);
    bell.current?.focus();
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
          aria-label={discreet ? "Show balance" : "Hide balance"}
          title={discreet ? "Show balance" : "Hide balance"}
          onClick={() => ui.set({ discreet: !discreet })}
        >
          <EyeIcon hidden={discreet} />
        </button>
        <button
          type="button"
          className="toolbar-button"
          data-testid="toolbar-lock"
          aria-label="Lock wallet"
          title="Lock wallet"
          disabled={!canLock}
          onClick={onLock}
        >
          <LockIcon />
        </button>
      </span>
    </header>
  );
}

const LOCK_TEXT: Record<LockStatus["state"], string> = {
  noVault: "No keys",
  noKeys: "Watch-only",
  unencrypted: "Not encrypted",
  locked: "Locked",
  unlockedMixingOnly: "Unlocked for mixing only",
  unlocked: "Unlocked",
};

export function StatusBar({
  sync,
  lock,
  unitName,
  hd,
  onUnlock,
}: {
  sync: SyncStatus | undefined;
  lock: LockStatus | undefined;
  unitName: string;
  hd: boolean;
  /** Opens the unlock sheet: the lock state is a button while unlocked for mixing only (U-02 D12). */
  onUnlock: () => void;
}) {
  const percent = sync && isSyncing(sync) ? syncPercent(sync) : null;
  return (
    <footer className="status-bar" data-testid="status-bar">
      <span className="status-left">
        <span role="status">{syncState(sync)}</span>
        {percent !== null && (
          <>
            <span aria-hidden="true">{percent.toFixed(1)}%</span>
            <progress className="status-progress" max={100} value={percent} aria-label="Sync progress" />
          </>
        )}
      </span>
      <span className="status-right">
        <span>{unitName}</span>
        {hd && <span>HD</span>}
        {lock?.state === "unlockedMixingOnly" ? (
          <button type="button" className="status-button" data-testid="status-unlock" title="Unlock fully" onClick={onUnlock}>
            {LOCK_TEXT[lock.state]}
          </button>
        ) : (
          lock && <span>{LOCK_TEXT[lock.state]}</span>
        )}
        {sync && <span>{sync.connectedPeers === 1 ? "1 peer" : `${sync.connectedPeers} peers`}</span>}
        {sync && <span>{!sync.running ? "Offline" : sync.caughtUp ? "Synced" : "Syncing"}</span>}
      </span>
    </footer>
  );
}
