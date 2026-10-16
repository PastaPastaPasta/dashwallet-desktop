// Overview (Home), UX-SPEC §4.5: hero band with the balance and breakdown strip, the shortcut
// card straddling its edge, the DashPay and CoinJoin cards (DP1-08, U-06), the frequent contacts
// (DP3-07), then the recent history in day-group cards.

import { useEffect } from "react";
import type { Balances } from "../api/generated/Balances";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { TxRow } from "../api/generated/TxRow";
import { mark } from "../app/startup";
import { navigate, openPayForm, ui, useUi } from "../app/ui";
import { useBackend, useBalance, useHistory, useSyncStatus, type HistoryFilter } from "../api/queries";
import { AmountText, DashButton, Icon, NetworkCapsule } from "../components/basics";
import { EmptyState, LoadingState, TxGroups, useRowContext } from "../components/history";
import type { UsedIcon } from "../components/icons";
import { useContactsOwner } from "../contacts/queries";
import { useStatusInputs } from "../dashpay/queries";
import { homeCard, type CardActionKind } from "../dashpay/status";
import { useDisplay } from "../format/display";
import { isSyncing, syncPercent } from "../format/sync";
import { CoinJoinCard } from "./coinjoin/CoinJoinCard";
import { DashPayCard, defaultCardAction } from "./dashpay/DashPayCard";
import { buyCredits } from "./dashpay/MyProfile";
import { useFrequentContacts } from "./pay/contactQueries";
import { FrequentStrip } from "./pay/FrequentStrip";

/** dash-qt's overview shows this many recent transactions. */
const RECENT_COUNT = 5;
const RECENT_FILTER: HistoryFilter = { categories: [], text: null, sort: "newestFirst" };

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

function BalanceHero({ balances, network, syncing }: { balances: Balances | null; network: NetworkInfo; syncing: boolean }) {
  const discreet = useUi((s) => s.discreet);
  const shown = useDisplay(network);
  const fmt = (duffs: number) => (discreet ? shown.masked() : shown.floored(duffs));
  // dash-qt's `.withUnit` tooltip on amounts (UX-SPEC §5.1); none while masked.
  const full = (duffs: number) => (discreet ? undefined : shown.withUnit(duffs));
  const unit = shown.name;
  return (
    <section className="hero" data-testid="hero" aria-label="Balance">
      {network.kind === "mainnet" ? <span className="network-capsule-slot" /> : <NetworkCapsule network={network} testId="hero-capsule" />}
      <span className="hero-caption" aria-hidden={!syncing}>
        {syncing ? "Syncing Balance" : " "}
      </span>
      {balances ? (
        <button
          type="button"
          className="hero-amount"
          data-testid="hero-amount"
          title={discreet ? "Click to show balance" : "Click to hide balance"}
          onClick={() => ui.set({ discreet: !discreet })}
        >
          <AmountText text={fmt(balances.total)} unitName={unit} title={full(balances.total)} />
        </button>
      ) : (
        <div className="hero-amount hero-unknown" data-testid="hero-amount" title="Not available until sync completes">
          <span className="amount-number">—</span>
          <span className="hero-unknown-caption">Balance unavailable</span>
        </div>
      )}
      {balances && (
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

/** Pay ▸ To an address, a fresh form (as a link would open it). */
function openPay(focusAddress: boolean) {
  openPayForm();
  if (focusAddress) requestAnimationFrame(() => document.querySelector<HTMLElement>('[data-testid="pay-address"]')?.focus());
}

// U-03 D16: the mobile shortcut bar. The desktop has no camera, so Scan QR opens the address
// field to paste into, as U-01 routes a scanQr link; Back up comes with the recovery phrase (U-02).
const SHORTCUTS: { id: string; icon: UsedIcon; title: string; run: (() => void) | null; why?: string }[] = [
  { id: "receive", icon: "shortcutReceive", title: "Receive", run: () => navigate("receive") },
  { id: "send", icon: "shortcutSend", title: "Send", run: () => openPay(false) },
  { id: "scan", icon: "shortcutScanQR", title: "Scan QR", run: () => openPay(true) },
  { id: "backup", icon: "shortcutBackup", title: "Back up", run: null, why: "Backing up the recovery phrase is not available yet" },
];

function ShortcutCard() {
  return (
    <nav className="shortcut-card" aria-label="Shortcuts" data-testid="shortcut-card">
      {SHORTCUTS.map((s) => (
        <button
          key={s.id}
          type="button"
          className="shortcut-item"
          data-testid={`shortcut-${s.id}`}
          aria-disabled={s.run ? undefined : "true"}
          title={s.why}
          onClick={() => s.run?.()}
        >
          <Icon id={s.icon} size={46} className="shortcut-icon" />
          <span className="shortcut-title">{s.title}</span>
        </button>
      ))}
    </nav>
  );
}

/** DP1-08: the DashPay status card (D13), none in the steady state (D2). */
function HomeDashPayCard({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const b = useBackend();
  const { inputs, retry } = useStatusInputs(walletId, network);
  const card = homeCard(inputs);
  // DP1-10 D8: "Buy credits" opens My Profile with the top-up sheet.
  const onAction = (kind: CardActionKind) => (kind === "buyCredits" && inputs.main ? buyCredits(b, walletId, inputs.main) : defaultCardAction(kind, retry));
  return card && <DashPayCard card={card} onAction={onAction} />;
}

/** DP3-07 D7: the main identity's frequent contacts (DP3-06's strip); a pick opens Pay ▸ To a
 * contact on them, as the contact pane's Pay does. Nothing without an owner. */
function HomeFrequent({ walletId }: { walletId: string }) {
  const { owner } = useContactsOwner(walletId);
  return owner.kind === "identity" && <HomeFrequentOf key={owner.identity} walletId={walletId} identity={owner.identity} />;
}

function HomeFrequentOf({ walletId, identity }: { walletId: string; identity: string }) {
  const frequent = useFrequentContacts(walletId, identity);
  return (
    <FrequentStrip
      contacts={frequent.data ?? []}
      testId="home-frequent"
      className="home-frequent card-l1"
      onPick={(p) => navigate("pay", { payTab: "contact", payContact: { walletId, contact: p.contact } })}
    />
  );
}

export function Overview({ walletId, network, onOpen }: { walletId: string; network: NetworkInfo; onOpen: (row: TxRow) => void }) {
  const balance = useBalance(walletId);
  const sync = useSyncStatus();
  const recent = useHistory(walletId, RECENT_FILTER, RECENT_COUNT);
  const discreet = useUi((s) => s.discreet);
  const rows = recent.data?.pages[0]?.rows ?? [];
  const ctx = useRowContext(network);
  const syncing = !!sync.data && isSyncing(sync.data);
  const percent = sync.data ? syncPercent(sync.data) : null;

  // Cold start (U5): the first frame that shows the balance and history (`mark` is idempotent).
  useEffect(() => {
    if (!balance.isPending && !recent.isPending) mark("first-paint");
  }, [balance.isPending, recent.isPending]);

  return (
    <main className="page page-overview" data-testid="page-home">
      <div className="hero-band" data-testid="hero-band">
        <BalanceHero balances={balance.data ?? null} network={network} syncing={syncing} />
        <ShortcutCard />
      </div>
      <div className="page-column">
        <HomeDashPayCard walletId={walletId} network={network} />
        <CoinJoinCard walletId={walletId} network={network} />
        {/* Who the wallet pays most is payment history: discreet mode hides it too (D8). */}
        {!discreet && <HomeFrequent walletId={walletId} />}
        <div className="history-header" data-testid="history-header">
          <h2 className="history-title">History</h2>
          <span className="history-actions">
            {syncing && percent !== null && <span className="history-syncing">Syncing {percent.toFixed(1)}%</span>}
            <DashButton variant="plainBlue" size="small" onClick={() => navigate("activity")}>
              Filter
            </DashButton>
          </span>
        </div>
        {discreet ? (
          <EmptyState testId="history-hidden" title="Recent transactions are hidden in discreet mode" />
        ) : recent.isPending ? (
          <LoadingState testId="history-loading" text="Loading transactions" />
        ) : rows.length === 0 ? (
          <EmptyState
            testId="history-empty"
            title="There are no transactions to display"
            action={
              <DashButton variant="filledBlue" data-testid="history-empty-receive" onClick={() => navigate("receive")}>
                Receive Dash
              </DashButton>
            }
          />
        ) : (
          <>
            <TxGroups rows={rows} ctx={ctx} onOpen={onOpen} testIdPrefix="overview" />
            <div className="see-all" data-testid="see-all">
              <DashButton variant="plainBlue" size="medium" onClick={() => navigate("activity")}>
                See all transactions
              </DashButton>
            </div>
          </>
        )}
      </div>
    </main>
  );
}
