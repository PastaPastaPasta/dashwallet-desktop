// Activity (UX-SPEC §4.9, U-03 D12/D13): search, the iOS type chips, dash-qt's date filter and
// minimum amount; List mode (day-group cards, the default) or Table mode (dash-qt's columns,
// sorted by the engine); pages of 200 records loaded as the list scrolls.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import type { HistorySortName } from "../api/generated/HistorySortName";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { TxCategoryName } from "../api/generated/TxCategoryName";
import type { TxRow } from "../api/generated/TxRow";
import { errorMessage } from "../api/backend";
import { useBackend, useHistory, usePlatformActivity, useShieldedHistory, type HistoryFilter } from "../api/queries";
import { fileErrorText } from "../app/files";
import { useLocalDay } from "../app/localDay";
import { useDebounced } from "../app/useDebounced";
import { DashButton, Icon } from "../components/basics";
import { EmptyState, LoadingState, TxGroups, TxTable, useRowContext, type RowDecor } from "../components/history";
import { UNIT_LABEL, useDisplay } from "../format/display";
import { exportHistory } from "../history/exportCsv";
import { filterExtra, isMayLandRow, isReplacedRow, mergeActivity, platformRow, rowSource, shieldedRow, SOURCE_TAG, SourceRowText, sourceRowTitle } from "../history/sourceRows";
import { DATE_CHOICES, dateRange, type DateChoice } from "../format/tx";

const PAGE_SIZE = 200;
const SEARCH_DEBOUNCE_MS = 250;

/** A Platform or Shielded row among the Core ones: titled and tagged, and it opens nothing. */
const decorateSource = (row: TxRow): RowDecor => {
  const source = rowSource(row);
  if (!source) return {};
  const replaced = isReplacedRow(row);
  return {
    title: sourceRowTitle(row),
    tag: SOURCE_TAG[source],
    readOnly: true,
    ...(replaced ? { note: SourceRowText.replaced, hideAmount: true } : isMayLandRow(row) ? { note: SourceRowText.mayLand } : {}),
  };
};

const CATEGORIES: { id: string; title: string; categories: TxCategoryName[] }[] = [
  { id: "all", title: "All", categories: [] },
  { id: "sent", title: "Sent", categories: ["sent"] },
  { id: "received", title: "Received", categories: ["received"] },
  { id: "mixing", title: "Mixing", categories: ["coinJoin"] },
  { id: "rewards", title: "Rewards", categories: ["reward"] },
];

type Mode = "list" | "table";

export function Transactions({ walletId, network, onOpen }: { walletId: string; network: NetworkInfo; onOpen: (row: TxRow) => void }) {
  const backend = useBackend();
  const [mode, setMode] = useState<Mode>("list");
  const [category, setCategory] = useState(CATEGORIES[0]);
  const [search, setSearch] = useState("");
  const [date, setDate] = useState<DateChoice>("all");
  const [minText, setMinText] = useState("");
  const [sort, setSort] = useState<HistorySortName>("newestFirst");
  const text = useDebounced(search.trim(), SEARCH_DEBOUNCE_MS);
  const min = useDebounced(minText.trim(), SEARCH_DEBOUNCE_MS);
  const shown = useDisplay(network);
  const parsedMin = useQuery({
    queryKey: ["parseAmount", shown.unit, min],
    queryFn: () => backend.parseAmount(min, shown.unit),
    enabled: min !== "",
    staleTime: Infinity,
  });
  const minAmount = min !== "" && typeof parsedMin.data === "number" && parsedMin.data > 0 ? parsedMin.data : undefined;
  const minError = min !== "" && parsedMin.data === null;
  // The date bounds come from the backend's clock (fixed in fixtures), taken again when the choice
  // changes and when the local day does (midnight, or the window coming back: D25).
  const clock = useCallback(() => backend.now(), [backend]);
  const day = useLocalDay(clock);
  const range = useMemo(() => dateRange(date, clock()), [date, clock, day]);
  // List mode always shows the newest first (it groups by day); Table mode sorts.
  const order = mode === "table" ? sort : "newestFirst";
  // Query keys are hashed structurally, so a fresh filter object per render is fine.
  const filter: HistoryFilter = { categories: category.categories, text: text || null, sort: order, ...range, ...(minAmount ? { minAmount } : {}) };
  const history = useHistory(walletId, filter, PAGE_SIZE);
  const coreRows = useMemo(() => history.data?.pages.flatMap((p) => p.rows) ?? [], [history.data]);
  // Platform-address and Shielded activity join the list under "All" with no search text (they
  // have no type to filter by, and nothing to search); Table mode is the Core columns.
  const platform = usePlatformActivity(walletId);
  const shielded = useShieldedHistory(walletId);
  const extra = useMemo(() => [...(platform.data ?? []).map(platformRow), ...(shielded.data ?? []).map(shieldedRow)], [platform.data, shielded.data]);
  const withExtra = mode === "list" && category.id === "all" && !text;
  const extraShown = useMemo(
    () => (withExtra ? filterExtra(extra, { dateFrom: range.dateFrom, dateTo: range.dateTo, minAmount }) : []),
    [withExtra, extra, range.dateFrom, range.dateTo, minAmount],
  );
  const rows = useMemo(() => mergeActivity(coreRows, !!history.hasNextPage, extraShown), [coreRows, history.hasNextPage, extraShown]);
  const coreTotal = history.data?.pages[0]?.totalMatching ?? null;
  // Every extra row the filters keep counts, loaded or not; Export is the Core history only.
  const total = coreTotal === null ? null : coreTotal + extraShown.length;
  const ctx = useRowContext(network);

  // Export CSV (QT-093): the records the filters select, all pages; the line under the bar says
  // where the file went, or why not.
  const [exporting, setExporting] = useState(false);
  const [exported, setExported] = useState<{ ok: boolean; text: string } | null>(null);
  const exportCsv = async () => {
    setExporting(true);
    setExported(null);
    try {
      const path = await exportHistory(backend, walletId, filter, shown.unit, backend.now());
      if (path !== null) setExported({ ok: true, text: `Exported to ${path}.` });
    } catch (e) {
      setExported({ ok: false, text: `The history could not be exported. ${fileErrorText(e)}` });
    } finally {
      setExporting(false);
    }
  };

  // Load the next page when the end of the list scrolls into view. After a failed page the
  // observer stays off until the user retries, so a persistent error cannot loop.
  const sentinel = useRef<HTMLDivElement>(null);
  const { hasNextPage, isFetchingNextPage, isFetchNextPageError, fetchNextPage } = history;
  useEffect(() => {
    const el = sentinel.current;
    if (!el || !hasNextPage || isFetchNextPageError) return;
    const io = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting) && !isFetchingNextPage) void fetchNextPage();
    });
    io.observe(el);
    return () => io.disconnect();
  }, [hasNextPage, isFetchingNextPage, isFetchNextPageError, fetchNextPage]);

  const exportWhy = exporting ? "Exporting…" : !coreTotal ? "There are no transactions to export yet." : null;
  return (
    <main className={`page page-transactions ${mode === "table" ? "is-table" : ""}`} data-testid="page-activity">
      <div className="page-column">
        <header className="top-intro top-intro-row" data-testid="tx-intro">
          <h1 className="top-intro-title">Activity</h1>
          <div className="tx-intro-actions">
            <DashButton variant="tintedGray" size="small" data-testid="tx-export-csv" disabled={exporting || !coreTotal} why={exportWhy} whyId="tx-export-why" onClick={() => void exportCsv()}>
              Export CSV
            </DashButton>
            <div className="segmented" role="group" aria-label="View" data-testid="tx-mode">
              {(["list", "table"] as const).map((m) => (
                <button key={m} type="button" className="segment" aria-pressed={mode === m} data-testid={`tx-mode-${m}`} onClick={() => setMode(m)}>
                  {m === "list" ? "List" : "Table"}
                </button>
              ))}
            </div>
          </div>
        </header>
        {exportWhy && (
          <p className="field-hint" id="tx-export-why" data-testid="tx-export-why">
            {exportWhy}
          </p>
        )}
        {exported && (
          <p className={exported.ok ? "settings-done" : "form-problem"} role={exported.ok ? "status" : "alert"} data-testid="tx-export-result">
            {exported.text}
          </p>
        )}
        <div className="tx-filter-bar" data-testid="tx-filter-bar">
          <div className="tx-filter-row">
            <label className="search-field" data-testid="tx-search">
              <Icon id="searchMagnifyingGlass" size={16} className="search-icon" />
              <input
                type="search"
                value={search}
                placeholder="Search address, transaction id or label"
                aria-label="Search address, transaction id or label"
                onChange={(e) => setSearch(e.target.value)}
              />
            </label>
            <label className="filter-select">
              <span>Date</span>
              <select data-testid="tx-date" value={date} onChange={(e) => setDate(e.target.value as DateChoice)}>
                {DATE_CHOICES.map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.title}
                  </option>
                ))}
              </select>
            </label>
            <label className="filter-select">
              <span>Min amount</span>
              <input
                data-testid="tx-min-amount"
                className="filter-amount"
                value={minText}
                inputMode="decimal"
                placeholder={shown.placeholder}
                aria-invalid={minError}
                aria-describedby={minError ? "tx-min-amount-error" : undefined}
                onChange={(e) => setMinText(e.target.value)}
              />
              {minError && (
                <span id="tx-min-amount-error" className="field-error" data-testid="tx-min-amount-error">
                  Not an amount in {UNIT_LABEL[shown.unit]}: ignored
                </span>
              )}
            </label>
          </div>
          <div className="filter-chips" role="group" aria-label="Transaction type" data-testid="tx-categories">
            {CATEGORIES.map((c) => (
              <button
                key={c.id}
                type="button"
                aria-pressed={category === c}
                className={`filter-chip ${category === c ? "is-selected" : ""}`}
                data-testid={`tx-category-${c.id}`}
                onClick={() => setCategory(c)}
              >
                {c.title}
              </button>
            ))}
          </div>
        </div>
        {history.isPending ? (
          <LoadingState testId="tx-loading" text="Loading transactions" />
        ) : history.isError && coreRows.length === 0 ? (
          <EmptyState testId="tx-error" title="Could not load transactions" message={errorMessage(history.error)} />
        ) : rows.length === 0 ? (
          <EmptyState testId="tx-empty" title="There are no transactions to display" />
        ) : mode === "list" ? (
          <div data-testid="tx-list">
            <TxGroups rows={rows} ctx={ctx} onOpen={onOpen} testIdPrefix="tx" decorate={decorateSource} />
          </div>
        ) : (
          <TxTable rows={rows} ctx={ctx} onOpen={onOpen} sort={sort} onSort={setSort} />
        )}
        <div ref={sentinel} className="tx-sentinel">
          {isFetchingNextPage && <LoadingState testId="tx-loading-more" text="Loading more transactions…" />}
          {isFetchNextPageError && (
            <p className="tx-more-error" role="alert">
              Could not load more transactions: {errorMessage(history.error)}{" "}
              <DashButton variant="plainBlue" size="small" onClick={() => void fetchNextPage()}>
                Try again
              </DashButton>
            </p>
          )}
        </div>
        {total !== null && (
          <footer className="tx-footer" data-testid="tx-footer">
            {total === 1 ? "1 transaction" : `${total.toLocaleString(backend.locale)} transactions`}
          </footer>
        )}
      </div>
    </main>
  );
}
