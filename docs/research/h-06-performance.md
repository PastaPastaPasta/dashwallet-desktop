# H-06 performance: contacts, history, wallet open, memory

ROADMAP H-06 asks for a large wallet with 500 contacts, a 10k-row history, a cold restore with DashPay
and a contacts query under 100 ms at 1,000 contacts, with the U5 budgets (DASHPAY §2.1) still holding.
This page records the engine's numbers on a shared, loaded Linux VM, what was fixed, and what is left for 1.0.x.

ADR 0003 left the engine's share of U5 to "E-HIST" and to H-06: `history_page(200)` took 60–503 ms in
the engine and wallet open 2–9 s. E-HIST never landed (no commit, no ROADMAP row); nothing changed
the history read path between the gate and this work.

## Summary

| Metric | Budget | Before | After | Load | Verdict |
|---|---|---|---|---|---|
| Contacts query, 1,000 contacts, all sections | < 100 ms | — | p50 **34 ms**, p95 **47 ms** wall (CPU p50 33 ms) | load 70 | **pass** |
| Contacts query, 1,000, sort by last activity | < 100 ms | — | p50 42 ms, p95 69 ms (CPU p50 41 ms) | load 70 | **pass** |
| Contacts query, 1,000, search (hit / no hit) | < 100 ms | — | p50 31–33 ms, p95 45–47 ms | load 70 | **pass** |
| Contacts query, 500 contacts (all four shapes) | — | — | p50 15–17 ms, p95 21–24 ms | load 70 | pass |
| `history_page(200)`, 40,053 records, first page | p95 ≤ 20 ms | p50 418–440 ms, p95 619–850 ms (CPU p50 386 ms) | p50 **92 ms**, p95 **120 ms** (CPU p50 93 ms) | load 50–100 | **miss** (4.6×) |
| `history_page(200)`, middle page (cursor at record 20,000) | p95 ≤ 20 ms | p50 372–435 ms, p95 568–580 ms | p50 96 ms, p95 131 ms | load 50–100 | **miss** |
| `history_page(200)`, text search | p95 ≤ 20 ms | p50 292–391 ms, p95 432–515 ms | p50 100 ms, p95 129 ms | load 50–100 | **miss** |
| `history_page`, user-space instructions per call (mean of the three shapes) | — | 0.89 × 10⁹ | **0.18 × 10⁹** (−79 %) | load-independent | — |
| Wallet open, 40k wallet (engine only) | ≤ 1.5 s (cold start) | 5.3–15.8 s wall, 7.3–9 s CPU | unchanged | load 41–104 | **miss** |
| Engine PSS after open and 3×(3+60) pages, idle 5 s | ≤ 400 MB with the UI | 292–348 MiB | **210 MiB** | load 50–100 | **miss** with the UI's 195–257 MiB (ADR 0003) |
| Cold load with DashPay: session open over 500 contacts and 1,350 payments | (within cold start) | — | 0.32–0.49 s wall, 0.30–0.46 s CPU | load 70 | pass |
| Cold load: first contacts read (builds the read model) | — | — | 9–21 ms | load 70 | pass |

The contacts budget holds with room. The U5 budgets do not hold for the engine on the 40k wallet:
`history_page` is 4.6× over after this work, wallet open 4–10× over, and engine memory plus the UI's own
cost passes 400 MB. Each has a 1.0.x entry under "Follow-ups".

## Machine and method

- **Machine.** a shared Linux VM, 48 vCPUs (Xeon E5-2696 v4, 2.2 GHz, AVX2, no
  SHA-NI), 160 GB RAM, Ubuntu 26.04. It is shared: load average 40–104 during these runs. Wall times
  carry that load; CPU time (the sum of every thread's on-CPU time from `/proc/self/task/*/schedstat`) and
  `perf stat -e instructions:u` do not. Medians and p95 are over repeated calls in one process after
  warm-up calls.
- **History and wallet open.** `rust/crates/dw-engine/tests/h06_perf.rs`, an ignored integration test
  through the public API, in a release build (thin LTO). It opens a copy of the G-02 regtest fixture
  (`regtest40k-dw.tar.gz` from commit `042695b9f`: 40,051 stored transactions, 40,053 history records,
  offline, `no_platform`). Per shape: 3 warm-up calls, then 30–150 measured calls (`DWD_H06_ITERS`).
  The middle page starts from the cursor reached by paging through half the records. The text search
  is `"yz"`.
- **Contacts.** `rust/crates/dw-engine/src/platform/contacts_perf_tests.rs`, ignored unit tests. They
  seed the wallet's DashPay overlay through the session's store as the library writes it (one owned
  identity; 80 % established contacts with 3 confirmed payments each, 10 % incoming, 10 % outgoing
  requests), reopen the session so the library loads it from `wallet.sqlite`, then add each contact's
  cached profile and its payments' mined transactions in memory, so every row has a display name and a
  last activity. `DashPay::contacts` is called through the facade: 5 warm-up calls, then 100. The
  crate's unit tests need debug assertions, so these ran in the dev profile with
  `CARGO_PROFILE_DEV_OPT_LEVEL=3` (dependencies at the workspace's opt-level 2). The same probe with
  dw-engine unoptimised gave p50 51–59 ms CPU at 1,000 contacts, also under budget.
- **Wallet open, split.** `dwcli list` on the fixture with `DWCLI_LOG=info` and temporary timers around
  each step of `NetworkSession` open (not committed).

## Findings

### `history_page`

Every call rebuilt the whole history: it copied the wallet's history map (40k entries, each with its
full transaction), re-derived every record (`records_of`), stringified every owned address with
base58check (two SHA-256s each) and stable-sorted 40k `(SortKey, TxRecord)` pairs. The profile before
the fixes was 30 % copying and freeing, 21 % base58check and SHA-256, and the rest building and sorting
records. `limit: 1` cost the same as `limit: 200` (ADR 0003 saw that too).

Three contained fixes, each its own commit:

1. **The chain view reads the history store in place** (`NetworkSession::with_view`). It now builds the
   view under the store's read lock instead of over a copy. Every caller's closure is a pure function
   of the view; none takes the store's lock again. 0.89 → 0.26 × 10⁹ instructions per call together with
   fix 2.
2. **Each owned address is encoded once per view** (`Owned::text`, a lazily filled `OnceLock`). A page
   showed the same few hundred owned addresses on 40k records and encoded one per record.
3. **A page selects its records instead of sorting all of them** (`history::page`). It filters, keeps
   the indices after the cursor, selects the first `limit` with `select_nth_unstable_by` and sorts only
   those, comparing borrowed keys. The cursor, `total_matching` and the order are unchanged; a new test
   walks every page of four sorts and five page sizes, over three input orders, against the full sort. The CSV export still sorts
   everything. 0.26 → 0.18 × 10⁹ instructions per call.

What remains (92 ms) is deriving 40k records per call (`records_of`, `status_of`, the owned-script and
history lookups), and its allocation and freeing. That is the read-model work under follow-up 1.

The view now holds the history store's read lock while the records are derived (about 90 ms on this
wallet), where it used to hold it while copying the map. A block's history update waits for it about as
long as it waited for the copy. A writer queued on that lock also holds back later readers, so two
concurrent `history_page` calls with a block arriving between them run one after the other; the read model
in #296 removes the long hold.

### Wallet open

`NetworkSession` open of the 40k wallet, `dwcli list`, three runs:

| Step | Wall |
|---|---|
| Engine start, app database, quorum context | 0.1 s |
| `SqlitePersister::load()` (platform-wallet-storage: wallets, pools, 40,082 coins, rehydrate) | 3.4–5.0 s |
| `restore_loaded_wallets` | 0.3 s |
| `load_history`: list the stored txids, then one `get_core_tx_record` point read and decode per transaction (40,051) | 1.3–3.1 s |
| Total (`startup session-open`) | 5.3–8.5 s |

CPU: 7.2–7.5 s user + 1.5–2.3 s system per `dwcli list`; 7.3–7.9 s CPU for the open in the probe.
`perf` puts 40 % of the samples in SQLite (`sqlite3VdbeExec` 21 %). The persister has no bulk record
read (`PlatformWalletPersistence` offers `get_core_tx_record` and `list_wallet_core_txids` only), so
batching the history load needs an upstream API: follow-up 2.

The fixture's open also logs "rewound the filter checkpoint for a replay" each time (its replay record
says 0 → 40,000); offline, no replay runs, and the numbers above do not include one.

### Memory

The probe process (the engine alone, no UI) after open and paging, idle 5 s: PSS 292 MiB after 30 calls
per shape and 348 MiB after 120 before the fixes (the per-call churn of 40k records fragments the heap),
262 MiB after fixes 1–2, 210 MiB after fix 3. ADR 0003's engine share at the gate was 186 MiB. With the
UI's own 195–257 MiB, the total is about 405–467 MiB (424–490 MB): over 400 MB. Follow-up 3.

### Contacts

1,000 contacts cost 33 ms at the median, linear in the number of contacts (500: 15 ms), so the budget
holds to about 2,500 contacts on this machine. Each read rebuilds the wallet's read model from the
library's state and re-derives each row: 27 % of the time is base58 of the contact ids, 16 % SHA-256 of
the provenance entities (the profile and both requests of each contact), the rest hex parsing of the
payments' txids and copies of their history entries for `last_activity_at`. A search costs the same as
the full list because every row's summary is built before the text filter. Follow-up 4.

A cold load of a wallet with 500 contacts and 1,350 payments in its DashPay overlay (the local half of a
restore with DashPay) opens the session in 0.32–0.49 s and serves the first contacts read in 9–21 ms. A
network restore (DashPay sync of 500 contacts' requests and profiles from Platform) was not measured: it
needs testnet identities with 500 contacts, which this offline probe does not have.

## Follow-ups (1.0.x)

1. **History read model kept between calls (E-HIST).** #296
   - Cause: `history_page` derives every record of the wallet on each call (0.18 × 10⁹ instructions,
     ~90 ms on 40k transactions), then pages them. `tx_detail`, the notices and the CSV export share the
     derivation.
   - Proposed fix: keep each wallet's records and a sort index in the session, updated from the history
     store's changes (upsert, forget, context and lock changes) and refreshed for depth-dependent status
     on a new tip. Join labels, fiat stamps and purposes per page from app.sqlite, and invalidate on their
     writes. A page is then O(limit + log n). Measure with `tests/h06_perf.rs`; target p95 ≤ 10 ms engine
     share on the 40k wallet.
2. **Wallet open: bulk record read and a lighter `load()`.** #297
   - Cause: `SqlitePersister::load()` takes 3.4–5.0 s on the 40k wallet, and `load_history` then reads
     each of the 40,051 records with its own SELECT (1.3–3.1 s). `stored_spends` does the same full read
     when a wallet is loaded later.
   - Proposed fix: an upstream platform-wallet-storage API that reads a wallet's records in one pass
     with the point read's checks (verified InstantSend locks, column drift); use it in `load_history` and
     `stored_spends`. Profile `load()` upstream (40k coins and their rehydrate). Consider serving the lock
     screen before the history load finishes.
3. **Engine memory on a 40k wallet.** #298
   - Cause: 210 MiB PSS for the engine alone after open; the history store keeps each transaction whole
     beside key-wallet's own records and coins.
   - Proposed fix: a heap profile (dhat) of an open 40k wallet; keep one copy of each transaction (share
     key-wallet's, or an `Arc`), and return freed pages after open (`malloc_trim`) or use a different
     allocator. Target: engine ≤ 150 MiB, so engine plus UI stays under 400 MB.
4. **Contacts read model cached between reads.** #299
   - Cause: each read rebuilds the wallet's contacts from the library and re-encodes every id and
     provenance digest (33 µs per contact); a search builds every row before filtering.
   - Proposed fix: keep the snapshot `read_contacts` already stores and rebuild it only when the library's
     state changed (the changeset tap's `Contacts`, `Identities` and `Payments` signals), with each row's
     Base58 id and entities computed once; filter on the stored names before building summaries.

## Reproduce

```sh
# The 40k fixture, unpacked (public regtest test data; passphrase "g02-regtest-fixture", not needed offline).
fixture=$(mktemp -d) && chmod 700 "$fixture"
git show 042695b9f:ci/gate/fixtures/regtest40k-dw.tar.gz | tar xz -C "$fixture"

cd rust
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0

# history_page, wallet open, memory (release)
DWD_H06_DATADIR="$fixture" DWD_H06_ITERS=60 \
  cargo test --release -p dw-engine --test h06_perf -- --ignored --nocapture

# contacts at 500 and 1,000, cold load of 500 (unit tests: debug assertions on, optimised)
CARGO_PROFILE_DEV_OPT_LEVEL=3 CARGO_PROFILE_TEST_OPT_LEVEL=3 \
  cargo test -p dw-engine --lib contacts_perf_tests -- --ignored --nocapture --test-threads=1

# instructions per history_page call: the difference between two iteration counts, over 3 × 40 calls
for n in 5 45; do
  DWD_H06_DATADIR="$fixture" DWD_H06_ITERS=$n perf stat -x, -e instructions:u \
    target/release/deps/h06_perf-<hash> --ignored --nocapture 2>&1 | grep instructions
done
```

Every line the probes print starts with `H06`. The contacts test asserts the median CPU time under
100 ms; the history probe asserts nothing, since it misses its budget.
