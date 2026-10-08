// The window shell (UX-SPEC §4.1, DASHPAY R11): toolbar, sidebar and status bar.

import type { ReactNode } from "react";
import type { LockStatus } from "../api/generated/LockStatus";
import type { SyncStatus } from "../api/generated/SyncStatus";
import { ui, useUi, type Page } from "../app/ui";
import { syncPercent } from "../format/sync";

const SIDEBAR_ICON: Record<string, ReactNode> = {
  // 18 pt outline glyphs in currentColor; macOS uses SF Symbols here (UX-SPEC §2.7).
  overview: <path d="M3 9.5 10 3.5l7 6V17a1 1 0 0 1-1 1h-4v-5H8v5H4a1 1 0 0 1-1-1V9.5Z" />,
  send: <path d="M5 15 15 5M8 5h7v7" />,
  receive: <path d="M15 5 5 15m7 0H5V8" />,
  transactions: <path d="M3 5h2m3 0h9M3 10h2m3 0h9M3 15h2m3 0h9" />,
};

interface SidebarItem {
  id: string;
  title: string;
  page?: Page;
  why?: string;
}

const ITEMS: SidebarItem[] = [
  { id: "overview", title: "Overview", page: "overview" },
  { id: "send", title: "Send", why: "Send is not part of the G-01 spike" },
  { id: "receive", title: "Receive", why: "Receive is not part of the G-01 spike" },
  { id: "transactions", title: "Transactions", page: "transactions" },
];

export function Sidebar() {
  const page = useUi((s) => s.page);
  return (
    <nav className="sidebar" aria-label="Main" data-testid="sidebar">
      <ul role="list" className="sidebar-list">
        {ITEMS.map((item) => {
          const current = item.page === page;
          return (
            <li key={item.id}>
              <button
                type="button"
                className={`sidebar-item ${current ? "is-current" : ""}`}
                data-testid={`sidebar-${item.id}`}
                aria-current={current ? "page" : undefined}
                aria-disabled={item.page ? undefined : true}
                title={item.why}
                onClick={() => item.page && ui.set({ page: item.page, detail: null })}
              >
                <svg className="sidebar-icon" viewBox="0 0 20 20" aria-hidden="true">
                  {SIDEBAR_ICON[item.id]}
                </svg>
                <span>{item.title}</span>
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

export function Toolbar({ walletName, canLock, onLock }: { walletName: string | null; canLock: boolean; onLock: () => void }) {
  const discreet = useUi((s) => s.discreet);
  return (
    <header className="toolbar" data-testid="toolbar">
      <span className="toolbar-wallet">{walletName}</span>
      <span className="toolbar-actions">
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

export function StatusBar({ sync, lock, unitName }: { sync: SyncStatus | undefined; lock: LockStatus | undefined; unitName: string }) {
  const percent = sync ? syncPercent(sync) : null;
  const left = !sync
    ? "Connecting…"
    : !sync.running
      ? "Not syncing"
      : sync.caughtUp
        ? "Up to date"
        : `Synchronizing with network… ${percent === null ? "" : `${percent.toFixed(1)}%`}`;
  return (
    <footer className="status-bar" data-testid="status-bar">
      <span className="status-left" role="status">
        {left}
        {sync && sync.running && !sync.caughtUp && percent !== null && (
          <progress className="status-progress" max={100} value={percent} aria-label="Sync progress" />
        )}
      </span>
      <span className="status-right">
        <span>{unitName}</span>
        <span>HD</span>
        {lock && <span>{LOCK_TEXT[lock.state]}</span>}
        {sync && <span>{sync.connectedPeers === 1 ? "1 peer" : `${sync.connectedPeers} peers`}</span>}
        {sync && <span>{!sync.running ? "Offline" : sync.caughtUp ? "Synced" : "Syncing"}</span>}
      </span>
    </footer>
  );
}
