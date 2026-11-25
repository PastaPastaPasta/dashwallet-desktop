// Overview (Home), UX-SPEC §4.5: hero band with the balance, its fiat value (MP-01) and breakdown
// strip, the shortcut card straddling its edge, the one-time price disclosure, the DashPay and
// CoinJoin cards (DP1-08, U-06), the frequent contacts
// (DP3-07), then the recent history in day-group cards.

import { Fragment, useEffect, useId, useState, type ReactNode } from "react";
import type { Balances } from "../api/generated/Balances";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { TxRow } from "../api/generated/TxRow";
import { mark } from "../app/startup";
import { navigate, openPayForm, ui, useUi } from "../app/ui";
import { errorMessage } from "../api/backend";
import { useBackend, useBalance, useHeroSync, useHistory, usePoolBalances, useSyncStatus, useWalletCatchUp, useWallets, type HistoryFilter } from "../api/queries";
import { AmountText, DashButton, Icon, NetworkCapsule, VisuallyHidden } from "../components/basics";
import { historyEmptyCopy, listDisplay, type ListArrival } from "../components/emptyStates";
import { EmptyState, LoadingState, TxGroups, useRowContext } from "../components/history";
import { Sheet } from "../components/sheet";
import type { UsedIcon } from "../components/icons";
import { useContactsOwner } from "../contacts/queries";
import { useStatusInputs } from "../dashpay/queries";
import { homeCard, type CardActionKind } from "../dashpay/status";
import { useDisplay } from "../format/display";
import { RatesConsentCard } from "../fiat/consent";
import { FiatLine, ratesNote, useFiat, type Fiat } from "../fiat/rates";
import { heldLine, heldWayOutText } from "../format/heldUnfinished";
import { breakdownRows, coreSubtitle, heroDigits, heroSyncLine, homeBalance, known, POOL_ROWS, type HeroSync, type PoolReading } from "../format/homeBalance";
import { isSyncing, oneDecimal, syncPercent } from "../format/sync";
import { CoinJoinCard } from "./coinjoin/CoinJoinCard";
import { DashPayCard, defaultCardAction, moveToDashWallet } from "./dashpay/DashPayCard";
import { identityCardAction } from "./dashpay/MyProfile";
import { useFrequentContacts } from "./pay/contactQueries";
import { FrequentStrip } from "./pay/FrequentStrip";
import { ExportStagingBanner } from "./ExportStagingBanner";
import { ProtocolAdvisoryBanner } from "./ProtocolAdvisoryBanner";
import { DroppedNoticeBanner } from "./DroppedNoticeBanner";
import { UpgradeReplayNotice } from "./UpgradeReplayNotice";
import { HistoryGapNotice } from "./HistoryGapNotice";
import { TransferSheet, useTransferPools } from "./TransferSheet";
import { UnfinishedTransfersCard } from "./UnfinishedTransfersCard";
import { defaultRoute, transferFor } from "../transfer/model";
import { t, useT } from "../i18n";

/** dash-qt's overview shows this many recent transactions. */
const RECENT_COUNT = 5;
const RECENT_FILTER: HistoryFilter = { categories: [], text: null, sort: "newestFirst" };


/** `amount` null: the read failed, shown as "—" with no unit; `undefined`: still being read. */
function BreakdownCell({ title, subtitle, amount, unitName, testId, fiat }: { title: string; subtitle: string; amount: string | null | undefined; unitName: string; testId: string; fiat: ReactNode }) {
  const tr = useT();
  return (
    <div className="breakdown-cell" data-testid={testId}>
      <span className="breakdown-text">
        <span className="breakdown-title">{title}</span>
        <span className="breakdown-subtitle">{subtitle}</span>
      </span>
      {amount === null || amount === undefined ? (
        <span className="breakdown-amount">
          <span aria-hidden="true">{amount === null ? "—" : "…"}</span>
          <VisuallyHidden>{amount === null ? tr("home.hero.notAvailable") : tr("home.hero.loading")}</VisuallyHidden>
        </span>
      ) : (
        <span className="breakdown-amounts">
          <AmountText className="breakdown-amount" text={amount} unitName={unitName} />
          {fiat}
        </span>
      )}
    </div>
  );
}

/** The coins a dropped payment holds back, and a link to how to get them back. Shown whenever any
 * coin is held, in discreet mode too (with the amount masked). */
function HeldLine({ text, walletId, awaiting }: { text: string; walletId: string; awaiting: boolean }) {
  const tr = useT();
  const wallets = useWallets();
  const [open, setOpen] = useState(false);
  const textId = useId();
  // The variant depends on how many wallets are open: none is chosen before the list is read.
  const wayOut = wallets.isPending ? null : heldWayOutText(walletId, wallets.data ?? [], awaiting);
  return (
    <>
      <p className="hero-held" data-testid="hero-held">
        <span className="hero-held-text" id={textId} data-testid="hero-held-text">
          {text}
        </span>
        <button type="button" className="hero-held-link" aria-describedby={textId} data-testid="hero-held-way-out" onClick={() => setOpen(true)}>
          {tr("home.core.heldWhatCanIDo")}
        </button>
      </p>
      {open && (
        <Sheet title={tr("home.core.heldWhatCanIDo")} width="small" testId="held-way-out" onClose={() => setOpen(false)}>
          {wayOut && (
            <p className="sheet-text" data-testid="held-way-out-text">
              {wayOut}
            </p>
          )}
        </Sheet>
      )}
    </>
  );
}

function BalanceHero({ walletId, balances, shielded, platform, network, hero }: { walletId: string; balances: Balances | null; shielded: PoolReading; platform: PoolReading; network: NetworkInfo; hero: HeroSync }) {
  const tr = useT();
  const discreet = useUi((s) => s.discreet);
  const shown = useDisplay(network);
  const fmt = (duffs: number) => (discreet ? shown.masked() : shown.compact(duffs));
  // dash-qt's `.withUnit` tooltip on amounts (UX-SPEC §5.1); none while masked.
  const full = (duffs: number) => (discreet ? undefined : shown.withUnit(duffs));
  const unit = shown.name;
  const home = balances && homeBalance(balances, shielded, platform);
  const rows = home ? breakdownRows(home.pools, discreet) : [];
  // Without a breakdown, what Core still has arriving or held goes under the hero (not in discreet mode).
  const coreNote = balances && rows.length === 0 && !discreet ? coreSubtitle(balances, fmt, "") : "";
  const held = balances ? heldLine(balances, fmt) : "";
  const digits = home ? heroDigits(fmt(home.total)) : null;
  const fiat = useFiat();
  return (
    <section className="hero" data-testid="hero" aria-label={tr("home.hero.label")} data-breakdown={home && rows.length === 0 ? "none" : undefined}>
      {network.kind === "mainnet" ? <span className="network-capsule-slot" /> : <NetworkCapsule network={network} testId="hero-capsule" />}
      <span className="hero-caption" role="status" data-testid="hero-sync-line">
        {heroSyncLine(balances, hero)}
      </span>
      {home && digits ? (
        <button
          type="button"
          className="hero-amount"
          data-testid="hero-amount"
          title={discreet ? tr("home.hero.show") : tr("home.hero.hide")}
          onClick={() => ui.set({ discreet: !discreet })}
        >
          <AmountText text={digits.major} minor={digits.minor} unitName={unit} title={full(home.total)} />
        </button>
      ) : (
        <div className="hero-amount hero-unknown" data-testid="hero-amount" title={tr("home.hero.unavailableTitle")}>
          <span className="amount-number">—</span>
          <span className="hero-unknown-caption">{tr("home.hero.unavailable")}</span>
        </div>
      )}
      {home && <HeroFiat fiat={fiat} duffs={home.total} hidden={discreet} />}
      {coreNote && (
        <p className="hero-note" data-testid="hero-core-note">
          {coreNote}
        </p>
      )}
      {home && balances && rows.length > 0 && (
        <div className="hero-breakdown" data-testid="hero-breakdown">
          {POOL_ROWS.filter((row) => rows.includes(row.pool)).map((row, i) => {
            const reading = home.pools[row.pool];
            // Core names what is still arriving or held; discreet mode keeps that to itself.
            const subtitle = row.pool === "core" && !discreet ? coreSubtitle(balances, fmt, row.subtitle) : row.subtitle;
            return (
              <Fragment key={row.pool}>
                {i > 0 && <span className="breakdown-divider" aria-hidden="true" />}
                <BreakdownCell
                  title={row.title}
                  subtitle={subtitle}
                  amount={known(reading) ? fmt(reading) : reading === "failed" ? null : undefined}
                  unitName={unit}
                  testId={`breakdown-${row.pool}`}
                  fiat={known(reading) && <FiatLine fiat={fiat} duffs={reading} hidden={discreet} className="breakdown-fiat" testId={`breakdown-${row.pool}-fiat`} />}
                />
              </Fragment>
            );
          })}
        </div>
      )}
      {held && balances && <HeldLine text={held} walletId={walletId} awaiting={balances.heldAwaiting} />}
    </section>
  );
}

/** The hero's fiat line and what is said about prices (MP-01): nothing on test networks or with
 * prices off, so the band keeps its height there. Discreet mode hides the number, not the note. */
function HeroFiat({ fiat, duffs, hidden }: { fiat: Fiat; duffs: number; hidden: boolean }) {
  const note = ratesNote(fiat.view);
  const text = hidden ? null : fiat.text(duffs);
  if (text === null && note === null) return null;
  return (
    <p className="hero-fiat" data-testid="hero-fiat-row">
      {text !== null && <FiatLine fiat={fiat} duffs={duffs} testId="hero-fiat" />}
      {note && (
        <span className={`hero-fiat-note is-${note.tone}`} data-testid="hero-rates-note" title={note.title}>
          {note.text}
        </span>
      )}
    </p>
  );
}

// U-03 D16: the Home shortcut bar; Back up comes with the recovery phrase (U-02).
// Transfer opens the sheet at Dash wallet → Shielded, or → Platform when Shielded is not active.
// Titles are read at render, so a language change shows at once.
const SHORTCUTS: { id: string; icon: UsedIcon; title: () => string; run: ((transfer: () => void) => void) | null; why?: () => string }[] = [
  { id: "receive", icon: "shortcutReceive", title: () => t("home.shortcuts.receive"), run: () => navigate("receive") },
  { id: "send", icon: "shortcutSend", title: () => t("home.shortcuts.send"), run: () => openPayForm() },
  { id: "transfer", icon: "transfer", title: () => t("transfer.shortcut"), run: (transfer) => transfer() },
  { id: "backup", icon: "shortcutBackup", title: () => t("home.shortcuts.backup"), run: null, why: () => t("home.shortcuts.backupWhy") },
];

function ShortcutCard({ onTransfer }: { onTransfer: () => void }) {
  const tr = useT();
  return (
    <nav className="shortcut-card" aria-label={tr("home.shortcuts.label")} data-testid="shortcut-card">
      {SHORTCUTS.map((s) => (
        <button
          key={s.id}
          type="button"
          className="shortcut-item"
          data-testid={`shortcut-${s.id}`}
          aria-disabled={s.run ? undefined : "true"}
          title={s.why?.()}
          onClick={() => s.run?.(onTransfer)}
        >
          <Icon id={s.icon} size={46} className="shortcut-icon" />
          <span className="shortcut-title">{s.title()}</span>
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
  // DP1-10 D8: "Buy credits" (and the voting card's Top up) opens My Profile with the top-up sheet.
  const onAction = (kind: CardActionKind) => {
    if (identityCardAction(b, walletId, inputs.main, kind, card)) return;
    return defaultCardAction(kind, retry, () => moveToDashWallet(b, walletId));
  };
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

/** Recent history with nothing in it yet. Only a caught-up wallet offers Receive. */
function HistoryEmpty({ arrival, recheck }: { arrival: ListArrival; recheck: () => void }) {
  const copy = historyEmptyCopy({ arrival, home: true });
  return (
    <EmptyState
      testId={arrival === "settled" ? "history-empty" : "history-syncing"}
      art={copy.art}
      title={copy.title}
      message={copy.message}
      action={
        copy.action && (
          <DashButton variant="filledBlue" data-testid={copy.action.testId} onClick={() => (arrival === "settled" ? navigate("receive") : recheck())}>
            {copy.action.label}
          </DashButton>
        )
      }
    />
  );
}

export function Overview({ walletId, network, onOpen }: { walletId: string; network: NetworkInfo; onOpen: (row: TxRow) => void }) {
  const tr = useT();
  const balance = useBalance(walletId);
  const pools = usePoolBalances(walletId);
  const sync = useSyncStatus();
  const { arrival, recheck } = useWalletCatchUp(walletId);
  const hero = useHeroSync(walletId);
  const recent = useHistory(walletId, RECENT_FILTER, RECENT_COUNT);
  const discreet = useUi((s) => s.discreet);
  const rows = recent.data?.pages[0]?.rows ?? [];
  const ctx = useRowContext(network);
  const historyList = listDisplay({
    rowCount: rows.length,
    pending: recent.isPending,
    failed: recent.isError,
    hasSnapshot: recent.data !== undefined,
    refreshing: recent.isFetching,
    arrival,
  });
  const syncing = !!sync.data && isSyncing(sync.data);
  const percent = sync.data ? syncPercent(sync.data) : null;
  const b = useBackend();
  const transferPools = useTransferPools(walletId, network);
  const transfer = transferFor(b, walletId);

  // Cold start (U5): the first frame that shows the balance and history (`mark` is idempotent).
  useEffect(() => {
    if (!balance.isPending && !recent.isPending) mark("first-paint");
  }, [balance.isPending, recent.isPending]);

  return (
    <main className="page page-overview" data-testid="page-home">
      <h1 className="sr-only">{tr("home.title")}</h1>
      <div className="hero-band" data-testid="hero-band">
        <BalanceHero walletId={walletId} balances={balance.data ?? null} shielded={pools.shielded} platform={pools.platform} network={network} hero={hero} />
        <ShortcutCard onTransfer={() => transfer.start(defaultRoute(transferPools))} />
      </div>
      <div className="page-column">
        <HistoryGapNotice />
        <UpgradeReplayNotice walletId={walletId} />
        <ProtocolAdvisoryBanner network={network} />
        <DroppedNoticeBanner walletId={walletId} network={network} />
        <ExportStagingBanner />
        <RatesConsentCard network={network} />
        <HomeDashPayCard walletId={walletId} network={network} />
        <UnfinishedTransfersCard walletId={walletId} network={network} />
        <TransferSheet walletId={walletId} network={network} />
        <CoinJoinCard walletId={walletId} network={network} />
        {/* Who the wallet pays most is payment history: discreet mode hides it too (D8). */}
        {!discreet && <HomeFrequent walletId={walletId} />}
        <div className="history-header" data-testid="history-header">
          <h2 className="history-title">{tr("home.history.title")}</h2>
          <span className="history-actions">
            {syncing && percent !== null && <span className="history-syncing">{tr("home.history.syncing", { percent: oneDecimal(percent) })}</span>}
            <DashButton variant="plainBlue" size="small" onClick={() => navigate("activity")}>
              {tr("home.history.filter")}
            </DashButton>
          </span>
        </div>
        {discreet ? (
          <EmptyState testId="history-hidden" title={tr("home.history.hidden")} />
        ) : historyList.kind === "failed" ? (
          <EmptyState
            testId="history-error"
            title={tr("home.history.failed")}
            message={errorMessage(recent.error)}
            action={
              <DashButton variant="plainBlue" data-testid="history-retry" onClick={() => void recent.refetch()}>
                {tr("home.history.tryAgain")}
              </DashButton>
            }
          />
        ) : historyList.kind === "arriving" && historyList.why === "pending" ? (
          <LoadingState testId="history-loading" text={tr("home.history.loading")} />
        ) : historyList.kind === "arriving" && historyList.why !== "pending" ? (
          <HistoryEmpty arrival={historyList.why} recheck={recheck} />
        ) : historyList.kind === "empty" ? (
          <HistoryEmpty arrival="settled" recheck={recheck} />
        ) : (
          <>
            <TxGroups rows={rows} ctx={ctx} onOpen={onOpen} testIdPrefix="overview" />
            <div className="see-all" data-testid="see-all">
              <DashButton variant="plainBlue" size="medium" onClick={() => navigate("activity")}>
                {tr("home.history.seeAll")}
              </DashButton>
            </div>
          </>
        )}
      </div>
    </main>
  );
}
