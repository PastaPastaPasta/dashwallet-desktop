// Overview (Home), UX-SPEC §4.5: hero band with the balance and breakdown strip, the shortcut
// card straddling its edge, then the recent history in day-group cards.

import { useEffect, useRef } from "react";
import { useBackend, useBalance, useHistory, useSyncStatus } from "../api/queries";
import type { Balances } from "../api/generated/Balances";
import type { HistoryRequest } from "../api/generated/HistoryRequest";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { TxRow } from "../api/generated/TxRow";
import { mark } from "../app/startup";
import { ui, useUi } from "../app/ui";
import { AmountText, DashButton, Icon } from "../components/basics";
import { EmptyState, LoadingState, TxGroups, type RowContext } from "../components/history";
import type { UsedIcon } from "../components/icons";
import { floored, masked } from "../format/amount";
import { syncPercent } from "../format/sync";

/** dash-qt's overview shows this many recent transactions. */
export const RECENT_COUNT = 5;
const RECENT_FILTER: Omit<HistoryRequest, "cursor" | "limit"> = { categories: [], text: null, sort: "newestFirst" };

function NetworkCapsule({ network }: { network: NetworkInfo }) {
  if (network.kind === "mainnet") return <span className="network-capsule-slot" />;
  return (
    <span className="network-capsule" data-testid="hero-capsule">
      {network.kind === "devnet" ? network.dirName.toUpperCase() : network.kind.toUpperCase()}
    </span>
  );
}

function BreakdownCell({ title, subtitle, amount, unitName, testId }: { title: string; subtitle: string; amount: string; unitName: string; testId: string }) {
  return (
    <div className="breakdown-cell" data-testid={testId}>
      <span className="breakdown-text">
        <span className="breakdown-title">{title}</span>
        <span className="breakdown-subtitle">{subtitle}</span>
      </span>
      <AmountText className="breakdown-amount" text={amount} unitName={unitName} />
    </div>
  );
}

function BalanceHero({ balances, known, network, syncing }: { balances: Balances | null; known: boolean; network: NetworkInfo; syncing: boolean }) {
  const discreet = useUi((s) => s.discreet);
  const fmt = (duffs: number) => (discreet ? masked() : floored(duffs));
  const unit = network.unitName;
  return (
    <section className="hero" data-testid="hero" aria-label="Balance">
      <NetworkCapsule network={network} />
      <span className="hero-caption" aria-hidden={!syncing}>
        {syncing ? "Syncing Balance" : " "}
      </span>
      {known && balances ? (
        <button
          type="button"
          className="hero-amount"
          data-testid="hero-amount"
          title={discreet ? "Click to show balance" : "Click to hide balance"}
          onClick={() => ui.set({ discreet: !discreet })}
        >
          <AmountText text={fmt(balances.total)} unitName={unit} glyphFactor={0.7} />
        </button>
      ) : (
        <div className="hero-amount hero-unknown" data-testid="hero-amount" title="Not available until sync completes">
          <span className="amount-number">—</span>
          <span className="hero-unknown-caption">Balance unavailable</span>
        </div>
      )}
      {known && balances && (
        <div className="hero-breakdown" data-testid="hero-breakdown">
          <BreakdownCell title="Available" subtitle="Spendable now" amount={fmt(balances.confirmed)} unitName={unit} testId="breakdown-available" />
          <span className="breakdown-divider" aria-hidden="true" />
          <BreakdownCell title="Pending" subtitle="Awaiting confirmation" amount={fmt(balances.unconfirmed)} unitName={unit} testId="breakdown-pending" />
          {balances.immature > 0 && (
            <>
              <span className="breakdown-divider" aria-hidden="true" />
              <BreakdownCell
                title="Immature"
                subtitle="Mining/masternode rewards maturing"
                amount={fmt(balances.immature)}
                unitName={unit}
                testId="breakdown-immature"
              />
            </>
          )}
        </div>
      )}
    </section>
  );
}

const SHORTCUTS: { id: string; icon: UsedIcon; title: string; why: string }[] = [
  { id: "receive", icon: "shortcutReceive", title: "Receive", why: "Receive is not part of the G-01 spike" },
  { id: "send", icon: "shortcutSend", title: "Send", why: "Send is not part of the G-01 spike" },
  { id: "scan", icon: "shortcutScanQR", title: "Scan QR", why: "Scanning is not part of the G-01 spike" },
  { id: "backup", icon: "shortcutBackup", title: "Back up", why: "Backup is not part of the G-01 spike" },
];

function ShortcutCard() {
  return (
    <nav className="shortcut-card" aria-label="Shortcuts" data-testid="shortcut-card">
      {SHORTCUTS.map((s) => (
        <button key={s.id} type="button" className="shortcut-item" data-testid={`shortcut-${s.id}`} aria-disabled="true" title={s.why}>
          <Icon id={s.icon} size={46} className="shortcut-icon" />
          <span className="shortcut-title">{s.title}</span>
        </button>
      ))}
    </nav>
  );
}

export function Overview({ walletId, network, onOpen }: { walletId: string; network: NetworkInfo; onOpen: (row: TxRow) => void }) {
  const backend = useBackend();
  const balance = useBalance(walletId);
  const sync = useSyncStatus();
  const recent = useHistory(walletId, RECENT_FILTER, RECENT_COUNT);
  const discreet = useUi((s) => s.discreet);
  const rows = recent.data?.pages[0]?.rows ?? [];
  const ctx: RowContext = { unitName: network.unitName, locale: backend.locale, now: backend.now() };
  const syncing = !!sync.data?.running && !sync.data.caughtUp;
  const percent = sync.data ? syncPercent(sync.data) : null;

  // Cold start (U5): the first frame that shows the balance and history.
  const reported = useRef(false);
  useEffect(() => {
    if (reported.current || balance.isPending || recent.isPending) return;
    reported.current = true;
    mark("first-paint");
  }, [balance.isPending, recent.isPending]);

  return (
    <main className="page page-overview" data-testid="page-overview">
      <div className="hero-band" data-testid="hero-band">
        <BalanceHero balances={balance.data ?? null} known={balance.data != null} network={network} syncing={syncing} />
        <ShortcutCard />
      </div>
      <div className="page-column">
        <div className="history-header" data-testid="history-header">
          <h2 className="history-title">History</h2>
          <span className="history-actions">
            {syncing && percent !== null && <span className="history-syncing">Syncing {percent.toFixed(1)}%</span>}
            <DashButton variant="plainBlue" size="small" onClick={() => ui.set({ page: "transactions" })}>
              Filter
            </DashButton>
          </span>
        </div>
        {discreet ? (
          <EmptyState testId="history-hidden" title="Recent transactions are hidden in discreet mode" />
        ) : recent.isPending ? (
          <LoadingState testId="history-loading" text="Loading transactions" />
        ) : rows.length === 0 ? (
          <EmptyState testId="history-empty" title="There are no transactions to display" />
        ) : (
          <>
            <TxGroups rows={rows} ctx={ctx} onOpen={onOpen} testIdPrefix="overview" />
            <div className="see-all" data-testid="see-all">
              <DashButton variant="plainBlue" size="medium" onClick={() => ui.set({ page: "transactions" })}>
                See all transactions
              </DashButton>
            </div>
          </>
        )}
      </div>
    </main>
  );
}
