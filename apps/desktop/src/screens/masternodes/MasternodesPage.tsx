// Masternodes ▸ Governance (MG-09, UX-SPEC §4.25.1, §4.25.3): the clock and budget cards, the
// proposal list with search, filters and sort, the detail pane, and Mine. MG-11 adds the tab bar
// with Username voting (NamesTab.tsx, MG-11 D1); MG-10 adds My masternodes to it and keeps the
// open tab in the saved settings (MG-10 D1). The sheets
// (vote, create, numbers) live here, so a list that changes under an open sheet (a filter, a
// re-sort, a resync) keeps it (D31).

import { useMemo, useState } from "react";
import { errorMessage } from "../../api/backend";
import type { GovernanceInfo } from "../../api/generated/GovernanceInfo";
import type { MasternodesTab } from "../../api/generated/MasternodesTab";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { ProposalRow } from "../../api/generated/ProposalRow";
import type { VotingMasternode } from "../../api/generated/VotingMasternode";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { useBackend } from "../../api/queries";
import { useLaunch } from "../../app/App";
import { ui, useUi } from "../../app/ui";
import { useDebounced } from "../../app/useDebounced";
import { AmountText, DashButton, Icon } from "../../components/basics";
import { EmptyState, LoadingState } from "../../components/history";
import { showToast } from "../../components/toast";
import { useDisplay, type Display } from "../../format/display";
import {
  budgetView,
  clockView,
  createdToast,
  GovText,
  isEnded,
  listState,
  needsMyVote,
  paymentsSuffix,
  sortRows,
  SORTS,
  statusChip,
  marginText,
  youSummary,
  type ListFilter,
  type SortOrder,
} from "../../governance/model";
import { useGovernanceInfo, useGovernanceRefresh, useGovernanceSyncState, useMyProposals, useProposals, useTurnOnGovernance, useVotingByProposal } from "../../governance/queries";
import { useT } from "../../i18n";
import { MnText } from "../../masternodes/model";
import { useMasternodeListState } from "../../masternodes/queries";
import { savePrefs } from "../../settings/prefs";
import { Tabs, type Tab } from "../Routes";
import { MoreMenu, Notice, StatusChip, useNowSecs, VoteBar } from "./common";
import { CreateProposalSheet } from "./CreateProposalSheet";
import { MineList } from "./Mine";
import { MyMasternodes } from "./MyMasternodes";
import { RegisterSheet } from "./RegisterSheet";
import type { ProTxOperation } from "../../api/generated/ProTxOperation";
import { NamesTab } from "./NamesTab";
import { NumbersSheet } from "./NumbersSheet";
import { ProposalPane } from "./ProposalPane";
import { SharedSessionSheet, SharedStartSheet, type OpenSession } from "./Shared";
import { VoteSheet } from "./VoteSheet";

const SEARCH_DEBOUNCE_MS = 200;


export function MasternodesPage({ wallet, network }: { wallet: WalletSummary; network: NetworkInfo }) {
  const t = useT();
  const backend = useBackend();
  const launch = useLaunch();
  const tabs: Tab<MasternodesTab>[] = [
    { id: "myMasternodes", title: t("masternodes.myMasternodes.tab") },
    { id: "governance", title: t("masternodes.governance.tab") },
    { id: "names", title: t("masternodes.names.tab") },
  ];
  const saved = useUi((s) => s.prefs.masternodesTab);
  // Shown at once; the setting follows (a failed save keeps the page on the chosen tab this time).
  const [tab, setShownTab] = useState<MasternodesTab>(saved);
  const setTab = (t: MasternodesTab) => {
    setShownTab(t);
    void savePrefs(backend, launch, { masternodesTab: t }).catch(() => undefined);
  };
  const mnList = useMasternodeListState();
  const [showEnded, setShowEnded] = useState(false);
  const [filter, setFilter] = useState<ListFilter>("all");
  const [sheet, setSheet] = useState<"create" | "numbers" | null>(null);
  const [tracking, setTracking] = useState(false);
  /** The registration wizard: new, or reopened at Prove on a saved External registration. */
  const [register, setRegister] = useState<{ resume?: ProTxOperation } | null>(null);
  /** The proposal the vote sheet is open on, as it was when Vote was pressed. */
  const [voteOn, setVoteOn] = useState<ProposalRow | null>(null);
  const [shared, setShared] = useState<OpenSession | "start" | null>(null);
  const walletId = wallet.walletId;
  const sync = useGovernanceSyncState();
  const info = useGovernanceInfo();
  const mine = useMyProposals(walletId);
  const { turnOn, busy: turningOn } = useTurnOnGovernance();
  const refresh = useGovernanceRefresh();
  const shown = useDisplay(network);
  const state = sync.data ? listState(sync.data) : null;
  // Once a run reached `synced` the engine keeps answering reads, so a later resync or a lost
  // peer keeps the list under its notice (D36).
  const listed = state?.kind === "ready" || (state !== null && state.kind !== "disabled" && sync.data?.lastSyncedAt != null);

  // Without the list (governance off, syncing or failed) Mine still shows below (D32).
  const mineBelow = (mine.data?.length ?? 0) > 0 && <MineList walletId={walletId} network={network} mine={mine.data!} heading />;
  let body;
  if (sync.isPending || info.isPending) body = <LoadingState testId="governance-loading" text={t("masternodes.governance.page.loading")} />;
  else if (sync.isError || info.isError)
    body = <EmptyState testId="governance-error" title={t("masternodes.governance.page.readFailed")} message={errorMessage(sync.error ?? info.error)} />;
  else if (state?.kind === "disabled")
    body = (
      <>
        <Notice
          tone="info"
          testId="governance-disabled"
          action={
            <DashButton variant="filledBlue" size="small" data-testid="governance-turn-on" disabled={turningOn} onClick={turnOn}>
              {GovText.turnOn}
            </DashButton>
          }
        >
          <strong>{GovText.disabled}</strong> {GovText.disabledHelp}
        </Notice>
        {mineBelow}
      </>
    );
  else
    body = (
      <>
        <ClockCard info={info.data!} shown={shown} synced={listed} />
        {wallet.watchOnly && (
          <Notice tone="info" testId="governance-notice-watch-only">
            <strong>{GovText.watchOnly}</strong> {GovText.watchOnlyText}
          </Notice>
        )}
        {state?.kind === "syncing" && (
          <section className="card-l1 mg-loading" data-testid="governance-syncing">
            <LoadingState testId="governance-syncing-progress" text={t("masternodes.governance.sync.loading", { line: state.line })} />
          </section>
        )}
        {state?.kind === "failed" && (
          <Notice
            tone="warning"
            testId="governance-failed"
            action={
              <DashButton
                variant="filledBlue"
                size="small"
                data-testid="governance-retry"
                onClick={() => void backend.governanceResync().catch(() => undefined).then(() => refresh())}
              >
                {GovText.tryAgain}
              </DashButton>
            }
          >
            {state.text}
          </Notice>
        )}
        {listed ? (
          <ProposalsView wallet={wallet} network={network} info={info.data!} showEnded={showEnded} filter={filter} setFilter={setFilter} onVote={setVoteOn} />
        ) : (
          mineBelow
        )}
      </>
    );

  return (
    <main className="page page-masternodes" data-testid="page-masternodes">
      <div className="page-column">
        <header className="top-intro mg-header">
          <h1 className="top-intro-title">{GovText.title}</h1>
          {tab === "myMasternodes" && (
            <span className="mg-header-actions" data-testid="mn-actions">
              <MoreMenu
                testId="mn-page-more"
                items={[
                  { id: "track", title: t("masternodes.myMasternodes.track.menu"), onSelect: () => setTracking(true) },
                  { id: "keys", title: t("masternodes.keys.open"), onSelect: () => ui.set({ dialog: "mnKeys", mnKeysRole: null }) },
                ]}
              />
              <DashButton
                variant="filledBlue"
                size="small"
                data-testid="mn-set-up"
                disabled={wallet.watchOnly}
                why={wallet.watchOnly ? t("masternodes.register.watchOnly") : undefined}
                onClick={() => setRegister({})}
              >
                {t("masternodes.register.setUp")}
              </DashButton>
            </span>
          )}
          {/* Governance's actions; Username voting has none (MG-11 D2). */}
          {tab === "governance" && (
            <span className="mg-header-actions" data-testid="governance-actions">
              <MoreMenu
                testId="governance-more"
                items={[
                  { id: "ended", title: showEnded ? GovText.hideEnded : GovText.showEnded, onSelect: () => setShowEnded((s) => !s) },
                  ...(info.data ? [{ id: "numbers", title: GovText.numbers, onSelect: () => setSheet("numbers") }] : []),
                ]}
              />
              <DashButton
                variant="filledBlue"
                size="small"
                data-testid="governance-create"
                disabled={wallet.watchOnly}
                title={wallet.watchOnly ? GovText.createWatchOnly : undefined}
                onClick={() => setSheet("create")}
              >
                {GovText.create}
              </DashButton>
            </span>
          )}
          {tab === "myMasternodes" && !wallet.watchOnly && (
            <span className="mg-header-actions" data-testid="mn-actions">
              <DashButton variant="tintedBlue" size="small" data-testid="mn-shared" onClick={() => setShared("start")}>
                {t("masternodes.shared.headerButton")}
              </DashButton>
            </span>
          )}
        </header>
        <Tabs label={GovText.title} tabs={tabs} current={tab} onSelect={setTab} testId="masternodes-tabs">
          <p className="mg-muted mg-small mg-list-height" data-testid="mn-list-slot">
            {!mnList.isError && mnList.data?.synced && mnList.data.height !== null && <span data-testid="mn-list-height">{MnText.listHeight(mnList.data.height)}</span>}
          </p>
          {tab === "myMasternodes" ? (
            <MyMasternodes
              wallet={wallet}
              network={network}
              tracking={tracking}
              onTrackClose={() => setTracking(false)}
              onVote={() => {
                setFilter("needsVote");
                setTab("governance");
              }}
              onContinue={(op) => setRegister({ resume: op })}
            />
          ) : tab === "governance" ? (
            body
          ) : (
            <NamesTab wallet={wallet} network={network} />
          )}
        </Tabs>
      </div>
      {sheet === "numbers" && info.data && <NumbersSheet info={info.data} network={network} onClose={() => setSheet(null)} />}
      {sheet === "create" && (
        <CreateProposalSheet
          wallet={wallet}
          network={network}
          onClose={() => setSheet(null)}
          onCreated={(p) => {
            setSheet(null);
            setFilter("mine");
            showToast(createdToast(p));
          }}
          onShowMine={() => {
            setSheet(null);
            setFilter("mine");
          }}
        />
      )}
      {register && <RegisterSheet wallet={wallet} network={network} resume={register.resume} onClose={() => setRegister(null)} />}
      {voteOn && <VoteSheet row={voteOn} wallet={wallet} network={network} onClose={() => setVoteOn(null)} />}
      {shared === "start" && <SharedStartSheet wallet={wallet} network={network} onClose={() => setShared(null)} onOpen={setShared} />}
      {shared && shared !== "start" && <SharedSessionSheet key={shared.id} wallet={wallet} network={network} open={shared} onClose={() => setShared(null)} />}
    </main>
  );
}

/** §4.25.3's two cards: the cycle with the estimated cutoff, and the budget. */
function ClockCard({ info, shown, synced }: { info: GovernanceInfo; shown: Display; synced: boolean }) {
  const t = useT();
  const now = useNowSecs();
  // The list's own query (the current proposals, no search): the over-budget part is summed from it.
  const rows = useProposals("current", "", synced).data;
  const clock = clockView(info, now);
  const budget = budgetView(info, synced ? rows : undefined);
  const amount = (duffs: number) => <AmountText text={shown.compact(duffs)} unitName={shown.name} />;
  return (
    <section className="mg-clock card-l1" data-testid="governance-clock">
      <div>
        <h2 className="mg-clock-title" data-testid="governance-clock-title">
          {clock.title}
        </h2>
        <p className="mg-clock-sub">{clock.sub}</p>
        <div className="mg-cycle" aria-hidden="true">
          <div className="mg-cycle-done" style={{ width: `${clock.done}%` }} />
          <span className="mg-cycle-mark" style={{ left: `${clock.cutoff}%` }} />
        </div>
        <div className="mg-cycle-labels">
          {clock.labels.map((l) => (
            <span key={l}>{l}</span>
          ))}
        </div>
      </div>
      <div data-testid="governance-budget">
        <div className="mg-spread mg-small">
          <strong>{budget.title}</strong>
          {budget.synced && <span className="mg-muted">{t("masternodes.governance.budget.estimate")}</span>}
        </div>
        <div className="mg-budget-bar" aria-hidden="true">
          {budget.synced && (
            <>
              <span className="mg-budget-passing" style={{ width: `${budget.synced.passingPct}%` }} />
              <span className="mg-budget-over" style={{ width: `${budget.synced.overPct}%` }} />
            </>
          )}
        </div>
        {budget.synced ? (
          <>
            <div className="mg-small">
              {t.rich("masternodes.governance.budget.passing", { allocated: amount(budget.synced.allocated), budget: amount(budget.budget), count: budget.synced.passing })}
            </div>
            {budget.synced.overCount > 0 && (
              <div className="mg-muted mg-small">
                {t.rich("masternodes.governance.budget.over", { count: budget.synced.overCount, amount: amount(budget.synced.overAmount) })}
              </div>
            )}
          </>
        ) : (
          <>
            <div className="mg-small">{t.rich("masternodes.governance.budget.available", { amount: amount(budget.budget) })}</div>
            <div className="mg-muted mg-small">{t("masternodes.governance.budget.afterSync")}</div>
          </>
        )}
      </div>
    </section>
  );
}

const FILTERS: { id: ListFilter; key: "masternodes.governance.page.all" | "masternodes.governance.page.needsVote" | "masternodes.governance.mine.title" }[] = [
  { id: "all", key: "masternodes.governance.page.all" },
  { id: "needsVote", key: "masternodes.governance.page.needsVote" },
  { id: "mine", key: "masternodes.governance.mine.title" },
];

/** The synced list: search, filter, sort, rows and the selected proposal's pane. */
function ProposalsView({
  wallet,
  network,
  info,
  showEnded,
  filter,
  setFilter,
  onVote,
}: {
  wallet: WalletSummary;
  network: NetworkInfo;
  info: GovernanceInfo;
  showEnded: boolean;
  filter: ListFilter;
  setFilter: (f: ListFilter) => void;
  onVote: (row: ProposalRow) => void;
}) {
  const t = useT();
  const walletId = wallet.walletId;
  const shown = useDisplay(network);
  const [search, setSearch] = useState("");
  const text = useDebounced(search.trim(), SEARCH_DEBOUNCE_MS);
  const [sort, setSort] = useState<SortOrder>("closest");
  const [selected, setSelected] = useState<string | null>(null);
  const proposals = useProposals(showEnded ? "all" : "current", text, true);
  const mine = useMyProposals(walletId);
  const rows = useMemo(() => proposals.data ?? [], [proposals.data]);
  const hashes = useMemo(() => rows.map((r) => r.hash), [rows]);
  const voting = useVotingByProposal(walletId, hashes, true);

  const needs = rows.filter((r) => needsMyVote(r, voting.get(r.hash)));
  const listed = sortRows(filter === "needsVote" ? needs : rows, sort);
  const current = listed.find((r) => r.hash === selected) ?? listed[0] ?? null;
  const noKeys = hashes.length > 0 && hashes.every((h) => voting.get(h)?.length === 0);
  const count = { all: rows.length, needsVote: needs.length, mine: mine.data?.length ?? 0 };

  let list;
  if (filter === "mine") list = <MineList walletId={walletId} network={network} mine={mine.data ?? []} />;
  else if (proposals.isPending) list = <LoadingState testId="proposals-loading" text={t("masternodes.governance.page.proposalsLoading")} />;
  else if (proposals.isError) list = <EmptyState testId="proposals-error" title={t("masternodes.governance.page.proposalsFailed")} message={errorMessage(proposals.error)} />;
  else if (listed.length === 0)
    list = (
      <EmptyState
        testId="proposals-empty"
        title={
          text !== ""
            ? t("masternodes.governance.page.noMatch", { text })
            : filter === "needsVote"
              ? GovText.emptyNeedsVote
              : t("masternodes.governance.page.noneThisCycle")
        }
      />
    );
  else
    list = (
      <div className="contacts-split">
        <div className="contacts-list">
          <section className="tx-group card-l1">
            <ul className="mg-list" data-testid="proposal-list">
              {listed.map((r) => (
                <li key={r.hash}>
                  <ProposalRowButton row={r} nodes={voting.get(r.hash)} info={info} shown={shown} current={r.hash === current?.hash} onSelect={() => setSelected(r.hash)} />
                </li>
              ))}
            </ul>
          </section>
        </div>
        {current && (
          <ProposalPane
            row={current}
            nodes={voting.get(current.hash)}
            info={info}
            wallet={wallet}
            network={network}
            onVote={() => {
              setSelected(current.hash);
              onVote(current);
            }}
          />
        )}
      </div>
    );

  return (
    <>
      {noKeys && !wallet.watchOnly && (
        <Notice tone="info" testId="governance-browsing-only">
          <strong>{GovText.browsingOnly}</strong> {GovText.browsingOnlyText}
        </Notice>
      )}
      <div className="tx-filter-bar mg-toolbar" data-testid="governance-toolbar">
        <label className="search-field" data-testid="governance-search">
          <Icon id="searchMagnifyingGlass" size={16} className="search-icon" />
          <input
            type="search"
            value={search}
            placeholder={GovText.search}
            aria-label={GovText.search}
            onChange={(e) => setSearch(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape" && search !== "") {
                e.preventDefault();
                setSearch("");
              }
            }}
          />
        </label>
        <div className="segmented" role="group" aria-label={t("masternodes.governance.page.show")} data-testid="governance-filter">
          {FILTERS.map((f) => (
            <button key={f.id} type="button" className="segment" aria-pressed={filter === f.id} data-testid={`governance-filter-${f.id}`} onClick={() => setFilter(f.id)}>
              {t(f.key)} {count[f.id]}
            </button>
          ))}
        </div>
        {filter !== "mine" && (
          <label className="filter-select">
            <span>{GovText.sortBy}</span>
            <select data-testid="governance-sort" value={sort} onChange={(e) => setSort(e.target.value as SortOrder)}>
              {SORTS.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.title}
                </option>
              ))}
            </select>
          </label>
        )}
      </div>
      {list}
    </>
  );
}

function ProposalRowButton({
  row,
  nodes,
  info,
  shown,
  current,
  onSelect,
}: {
  row: ProposalRow;
  nodes: VotingMasternode[] | undefined;
  info: GovernanceInfo;
  shown: Display;
  current: boolean;
  onSelect: () => void;
}) {
  const t = useT();
  const you = youSummary(nodes);
  const ask = { amount: <AmountText text={shown.compact(row.paymentAmount)} unitName={shown.name} />, payments: paymentsSuffix(row.paymentsRequested) };
  const margin = marginText(row, info.passingThreshold);
  return (
    <button type="button" className="mg-row mg-row-wide" aria-current={current ? "true" : undefined} data-testid={`proposal-row-${row.name}`} onClick={onSelect}>
      <span className="mg-row-text">
        <span className="mg-row-title">{row.name}</span>
        <span className="mg-row-sub">
          {you ? t.rich("masternodes.governance.withYou", { ...ask, you }) : t.rich("masternodes.governance.ask", ask)}
        </span>
        {!isEnded(row) && <VoteBar tally={row.tally} threshold={info.passingThreshold} weight={info.validWeight} />}
      </span>
      <span className="mg-row-trailing">
        <StatusChip chip={statusChip(row.status, row.closeCall)} />
        {margin && <span className="mg-row-meta">{margin}</span>}
      </span>
    </button>
  );
}
