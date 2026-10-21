// Masternodes ▸ Governance (MG-09, UX-SPEC §4.25.1, §4.25.3): the clock and budget cards, the
// proposal list with search, filters and sort, the detail pane, and Mine. Until MG-10 adds My
// masternodes the page has no tab bar (D1). The sheets (vote, create, numbers) live here, so a
// list that changes under an open sheet (a filter, a re-sort, a resync) keeps it (D31).

import { useMemo, useState } from "react";
import { errorMessage } from "../../api/backend";
import type { GovernanceInfo } from "../../api/generated/GovernanceInfo";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { ProposalRow } from "../../api/generated/ProposalRow";
import type { VotingMasternode } from "../../api/generated/VotingMasternode";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { useBackend } from "../../api/queries";
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
import { MoreMenu, Notice, StatusChip, useNowSecs, VoteBar } from "./common";
import { CreateProposalSheet } from "./CreateProposalSheet";
import { MineList } from "./Mine";
import { NumbersSheet } from "./NumbersSheet";
import { ProposalPane } from "./ProposalPane";
import { VoteSheet } from "./VoteSheet";

const SEARCH_DEBOUNCE_MS = 200;

export function MasternodesPage({ wallet, network }: { wallet: WalletSummary; network: NetworkInfo }) {
  const [showEnded, setShowEnded] = useState(false);
  const [filter, setFilter] = useState<ListFilter>("all");
  const [sheet, setSheet] = useState<"create" | "numbers" | null>(null);
  /** The proposal the vote sheet is open on, as it was when Vote was pressed. */
  const [voteOn, setVoteOn] = useState<ProposalRow | null>(null);
  const walletId = wallet.walletId;
  const sync = useGovernanceSyncState();
  const info = useGovernanceInfo();
  const mine = useMyProposals(walletId);
  const { turnOn, busy: turningOn } = useTurnOnGovernance();
  const backend = useBackend();
  const refresh = useGovernanceRefresh();
  const shown = useDisplay(network);
  const state = sync.data ? listState(sync.data) : null;
  // Once a run reached `synced` the engine keeps answering reads, so a later resync or a lost
  // peer keeps the list under its notice (D36).
  const listed = state?.kind === "ready" || (state !== null && state.kind !== "disabled" && sync.data?.lastSyncedAt != null);

  // Without the list (governance off, syncing or failed) Mine still shows below (D32).
  const mineBelow = (mine.data?.length ?? 0) > 0 && <MineList walletId={walletId} network={network} mine={mine.data!} heading />;
  let body;
  if (sync.isPending || info.isPending) body = <LoadingState testId="governance-loading" text="Loading governance…" />;
  else if (sync.isError || info.isError)
    body = <EmptyState testId="governance-error" title="Couldn't read governance" message={errorMessage(sync.error ?? info.error)} />;
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
            <LoadingState testId="governance-syncing-progress" text={`Loading proposals from the network… ${state.line}`} />
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
          <span className="mg-header-actions">
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
        </header>
        {body}
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
        />
      )}
      {voteOn && <VoteSheet row={voteOn} wallet={wallet} network={network} onClose={() => setVoteOn(null)} />}
    </main>
  );
}

/** §4.25.3's two cards: the cycle with the estimated cutoff, and the budget. */
function ClockCard({ info, shown, synced }: { info: GovernanceInfo; shown: Display; synced: boolean }) {
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
          {budget.synced && <span className="mg-muted">from peers, an estimate</span>}
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
              {amount(budget.synced.allocated)} of {amount(budget.budget)} goes to {budget.synced.passing} passing{" "}
              {budget.synced.passing === 1 ? "proposal" : "proposals"}
            </div>
            {budget.synced.overCount > 0 && (
              <div className="mg-muted mg-small">
                {budget.synced.overCount} more {budget.synced.overCount === 1 ? "passes" : "pass"} the vote but {budget.synced.overCount === 1 ? "doesn't" : "don't"} fit (
                {amount(budget.synced.overAmount)})
              </div>
            )}
          </>
        ) : (
          <>
            <div className="mg-small">{amount(budget.budget)} available</div>
            <div className="mg-muted mg-small">What passing proposals take shows after sync</div>
          </>
        )}
      </div>
    </section>
  );
}

const FILTERS: { id: ListFilter; title: string }[] = [
  { id: "all", title: "All" },
  { id: "needsVote", title: "Needs my vote" },
  { id: "mine", title: "Mine" },
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
  else if (proposals.isPending) list = <LoadingState testId="proposals-loading" text="Loading proposals…" />;
  else if (proposals.isError) list = <EmptyState testId="proposals-error" title="Couldn't read the proposals" message={errorMessage(proposals.error)} />;
  else if (listed.length === 0)
    list = (
      <EmptyState
        testId="proposals-empty"
        title={text !== "" ? `No proposal matches “${text}”` : filter === "needsVote" ? GovText.emptyNeedsVote : "No proposals this cycle"}
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
      <div className="tx-filter-bar mg-toolbar">
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
        <div className="segmented" role="group" aria-label="Show" data-testid="governance-filter">
          {FILTERS.map((f) => (
            <button key={f.id} type="button" className="segment" aria-pressed={filter === f.id} data-testid={`governance-filter-${f.id}`} onClick={() => setFilter(f.id)}>
              {f.title} {count[f.id]}
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
  const you = youSummary(nodes);
  const margin = marginText(row, info.passingThreshold);
  return (
    <button type="button" className="mg-row mg-row-wide" aria-current={current ? "true" : undefined} data-testid={`proposal-row-${row.name}`} onClick={onSelect}>
      <span className="mg-row-text">
        <span className="mg-row-title">{row.name}</span>
        <span className="mg-row-sub">
          <AmountText text={shown.compact(row.paymentAmount)} unitName={shown.name} /> {paymentsSuffix(row.paymentsRequested)}
          {you && ` · ${you}`}
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
