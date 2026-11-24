Q75b: PR A, C1/C2 delta (36e886df5..27c577c0a), for the funds reviewer. Not pushed.
c1c2.diff sha256 967f3a97d455754b052b559dc3358dfc36a67c8d12ccd5448c92ea09ca3051ec

C1: PR A's hold notes are renamed from `dropped:` to `drop_holds:` (tx_actions.rs:135), so #739's "" overwrite of `dropped:X` can't erase X's holds. Every PR A reader and writer uses the constant; the only literal `dropped:` left is the collision test. delete_wallet_rows still removes them, because it drops the wallet's whole settings scope.
- Fail-first: an_empty_dropped_key_does_not_erase_a_drops_holds (ff-c1-collision.txt). Before the fix, a "" write to `dropped:<C>` erased the holds and D was credited after a restart and rescan.

C2: claims are left to #739's push-only rule.
(i) events.rs ~:1623-1636. A sweep where a swept transaction spends an output of `superseded_by` queues no children. A sweep whose swept transaction the history lacks is treated the same way, conservatively; a debug line for that case is coming in the C4 delta.
(ii) history.rs ~:618-636. orphaned_unmined skips an orphan whose missing parent is a claim the store dropped (Persisted.dropped, doubtful ones included).
(iii) Beyond the ruling: a `claim_swept:<txid>` note. #739 records nothing for a sweep, so after a restart the orphan rule would still drop S. A claim sweep now writes `claim_swept:<txid>`, and the load exempts those from the orphan rule.
  - A failed read makes the load incomplete, so nothing is dropped.
  - The note's only reader is orphaned_unmined, and all it can do there is skip a drop. It never frees or credits a coin.
  - RESIDUAL GAP: if the app stops between the store saving the sweep and the note write (or the write fails, which is only logged), the next load drops S as an orphan. The winner is then unknown, so nothing is freed and b stays held through `drop_holds:`. The result is against #739's "keep" verdict, but it is funds-safe.
  - PROPOSED CLOSE:
    - Write `claim_swept:` from the TransactionsSwept handler before the drop work, as now.
    - Have the load also exempt any orphan whose missing parent spends an output of a transaction the history marks superseded (#739's swept_claims test, run at load against the stored history), so the note is only a cache.
    - If the store can take the note in the same SQLite transaction as the sweep's save, do that instead. To be confirmed in the code before choosing; this would be a 1.0 follow-up unless you want it in PR A.
- Tests (T8 shape: a claim whose input script isn't push-only, S built on its change and coin b): a_swept_doubtful_claims_payment_is_left_to_the_claim_rule and a_payment_on_a_claim_the_store_dropped_is_not_taken_as_orphaned. S is kept and b held, in the session and after a restart. Before outputs: ff-c2-sweep-and-store.txt and ff-c2-pre.txt.

Local review (claude-opus-5-5 high): APPROVED WITH SUGGESTIONS, funds-safe as it stands.
- Medium, being fixed now: PR A's `LoadedHistory.dropped` and #739's `LoadedHistory.dropped` share a name with opposite meanings. PR A's is an orphan exemption, where a larger set is safer. #739's feeds abandon_on_dead, where a larger set is less safe. A conflict resolution that chains them would let #739 drop S on a doubtful claim. PR A's field is being renamed to `orphan_exempt` (pure rename); lane 1 should be told.
- Low: a sweep whose swept transaction the history lacks is never dropped by either rule. Its children stay pending, with their coins held. This is fail-closed; the debug line is added.
- Low: one claim in a mixed sweep batch blocks children for the whole batch. Conservative.

Runs at 27c577c0a: own_spend 159 passed, 0 failed. Full dw-engine --tests (tests-full-8.txt): one lib failure, platform::banner_tests::a_closed_session_keeps_no_expiry_timer, under a 584 s loaded run. It passed alone twice (banner-rerun-8.txt), and the run at 3430eb341 (before a clippy-only collapse) had no failures. clippy and fmt are clean.

C4 (abandon_gate in drop_dead_unmined, now that #539 is on main), the rename and the debug line are in progress. Their delta (c4.diff) and its fail-first will be added to this folder.
