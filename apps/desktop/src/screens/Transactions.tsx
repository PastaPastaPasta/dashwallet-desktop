// Transactions (UX-SPEC §4.9), list mode: search, the iOS filter categories, day-group cards
// and pages of 200 records loaded as the list scrolls. Table mode is not part of the spike.

import { useEffect, useMemo, useRef, useState } from "react";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { TxCategoryName } from "../api/generated/TxCategoryName";
import type { TxRow } from "../api/generated/TxRow";
import { useBackend, useHistory } from "../api/queries";
import { Icon } from "../components/basics";
import { EmptyState, LoadingState, TxGroups, type RowContext } from "../components/history";

export const PAGE_SIZE = 200;
const SEARCH_DEBOUNCE_MS = 250;

const CATEGORIES: { id: string; title: string; categories: TxCategoryName[] }[] = [
  { id: "all", title: "All", categories: [] },
  { id: "sent", title: "Sent", categories: ["sent"] },
  { id: "received", title: "Received", categories: ["received"] },
  { id: "mixing", title: "Mixing", categories: ["coinJoin"] },
  { id: "rewards", title: "Rewards", categories: ["reward"] },
];

function useDebounced<T>(value: T, ms: number): T {
  const [v, setV] = useState(value);
  useEffect(() => {
    const t = window.setTimeout(() => setV(value), ms);
    return () => window.clearTimeout(t);
  }, [value, ms]);
  return v;
}

export function Transactions({ walletId, network, onOpen }: { walletId: string; network: NetworkInfo; onOpen: (row: TxRow) => void }) {
  const backend = useBackend();
  const [category, setCategory] = useState("all");
  const [search, setSearch] = useState("");
  const text = useDebounced(search.trim(), SEARCH_DEBOUNCE_MS);
  const filter = useMemo(
    () => ({ categories: CATEGORIES.find((c) => c.id === category)!.categories, text: text || null, sort: "newestFirst" as const }),
    [category, text],
  );
  const history = useHistory(walletId, filter, PAGE_SIZE);
  const rows = useMemo(() => history.data?.pages.flatMap((p) => p.rows) ?? [], [history.data]);
  const total = history.data?.pages[0]?.totalMatching ?? null;
  const ctx: RowContext = { unitName: network.unitName, locale: backend.locale, now: backend.now() };

  // Load the next page when the end of the list scrolls into view.
  const sentinel = useRef<HTMLDivElement>(null);
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = history;
  useEffect(() => {
    const el = sentinel.current;
    if (!el || !hasNextPage) return;
    const io = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting) && !isFetchingNextPage) void fetchNextPage();
    });
    io.observe(el);
    return () => io.disconnect();
  }, [hasNextPage, isFetchingNextPage, fetchNextPage]);

  return (
    <main className="page page-transactions" data-testid="page-transactions">
      <div className="page-column">
        <header className="top-intro" data-testid="tx-intro">
          <h1 className="top-intro-title">Transactions</h1>
        </header>
        <div className="tx-filter-bar" data-testid="tx-filter-bar">
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
          <div className="filter-chips" role="radiogroup" aria-label="Transaction type" data-testid="tx-categories">
            {CATEGORIES.map((c) => (
              <button
                key={c.id}
                type="button"
                role="radio"
                aria-checked={category === c.id}
                className={`filter-chip ${category === c.id ? "is-selected" : ""}`}
                data-testid={`tx-category-${c.id}`}
                onClick={() => setCategory(c.id)}
              >
                {c.title}
              </button>
            ))}
          </div>
        </div>
        {history.isPending ? (
          <LoadingState testId="tx-loading" text="Loading transactions" />
        ) : history.isError ? (
          <EmptyState testId="tx-error" title="Could not load transactions" message={String((history.error as { message?: string }).message ?? history.error)} />
        ) : rows.length === 0 ? (
          <EmptyState testId="tx-empty" title="There are no transactions to display" />
        ) : (
          <div data-testid="tx-list">
            <TxGroups rows={rows} ctx={ctx} onOpen={onOpen} testIdPrefix="tx" />
          </div>
        )}
        <div ref={sentinel} className="tx-sentinel">
          {isFetchingNextPage && <LoadingState testId="tx-loading-more" text="Loading more transactions…" />}
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
