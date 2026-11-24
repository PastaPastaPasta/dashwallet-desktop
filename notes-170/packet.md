FS6 → PO: #170 PR A funds packet (cases 3 and 4), for your ruling. Nothing is pushed.

Head: 36e886df581f073029341db9562598dd0af33eb3, branch fix/170-case3-4-beaten-change, all commits signed.
Base: merge-base with main is 78ddde27aebe9c189b53be5019f4b9919bc77b22, which includes #746. Main is now 63b5fe10a, 101 commits ahead, and `git merge-tree` against it is clean.
Diff (origin/main...HEAD): 11 files, +1725/-57, dw-engine plus one PLAN-1.0.md line. Saved at /work/scratch/fs6/notes-170/prA-36e886df5.diff, sha256 cce27fbbbca376aae1cad16445a85ed0e39f41b1aa933f551d46a675519c43b3.

THE SAFETY RULE (the gate): nothing is freed unless the winner's inputs are known, and an InstantSend-only winner that isn't in the history frees nothing.
- Code: tx_actions.rs:1067. `known = complete && !unknown_lock && (winner mined || winner in the history)`. `freed` is empty unless known, and every coin not freed and not spent in a processed block goes into the `dropped:` holds note.
- Test 3: a_child_of_a_payment_an_unknown_locked_winner_swept_keeps_its_coin_out (own_spend_balance_tests.rs:2564). C goes. D stays out after its block is scanned again, and after a restart.
  - With `known` forced true it fails at :2483 (failfirst-known.txt).
  - It passes at the head.
- Unknown lock: a_child_dropped_beside_an_unknown_lock_frees_nothing. Fails before the fix with "rescanned: D" (ff-r2-unknown-lock-FF_R2_M1.txt). Passes at the head.

CASE 14 (InstantSend-locked payments a ChainLock beat). Both tests fail at the head with the fix removed, and pass with it (ff-case14-head.txt):
- a_locked_payment_a_chainlock_beat_under_a_child_claiming_its_own_coin_is_refused_as_locked: fixed by history.rs:475. A beaten locked payment gets no claim, but the walk doesn't go through it, so its children keep theirs. Removing that line alone fails with "the child claims D".
- a_locked_payment_a_chainlock_beat_is_not_abandoned_while_the_scan_replays: fixed by history.rs:561. A history winner must be mined or recorded by key-wallet. Removing that line alone fails at own_spend_disk_tests.rs:1790 with "replayed to A".
- Both removed: 13 passed, 2 failed. Both restored: 15 of 15 pass.
- About "the pre-fix commit": the original red run (tests-full-2, 2910 passed, 2 failed) was on an uncommitted tree, and the fix was squashed into bca9c5089. So the fail-first is shown by removing the fix at the head.

WHAT THE DROP EXCLUDES, AND WHERE (at 36e886df5):
1. history.rs:475: a beaten InstantSend-locked payment gets no claim, and its children keep theirs (case 14 test above).
2. history.rs:464: only unlocked, unmined children of a beaten payment are followed.
3. history.rs:561: a winner must be mined or recorded; a restored lock key-wallet doesn't hold is not a winner.
4. history.rs:572: a block beats a locked payment only under a ChainLock.
5. history.rs:593: the sweep's queued children skip locked, mined and abandoned transactions.
6. history.rs:624: the orphan rule skips locked transactions.
7. tx_actions.rs:987: a root must be unmined, not abandoned and not locked.
8. tx_actions.rs:990: no root with anything settled under it in any loaded wallet. This is #746's settled_all/add_wallet; no second rule.
9. tx_actions.rs:991: no root key-wallet holds settled (in a block, finalized, or InstantSend-locked in its records or lock set).
10. tx_actions.rs:905, :914-915: descendants exclude locked or mined entries and key-wallet InstantSend records.
11. tx_actions.rs:1033/:1045/:1057: a coin an unmined locked rival spends is held, and kept in the note, unless a ChainLocked spend beat that lock (two-rivals fix).
12. tx_actions.rs:1061: coins a processed block spent are left out of that hold, because the block spends them again on any rescan.
13. tx_actions.rs:1064: an unknown lock means the inputs are unknown, so nothing is freed.
14. tx_actions.rs:1770: the holds note also lists the coins from item 11.
15. tx_actions.rs:1690: nothing is dropped while another wallet is unread (incomplete load, or an open under way).
16. tx_actions.rs:1635/:1704: no detection while this wallet's history isn't loaded or loaded incomplete, and such a drop frees nothing.
17. tx_actions.rs:1791: if key-wallet refuses the drop, the drop's marks are deleted.

YOUR TWO PACKET ITEMS
(1) No freeing while the history is incomplete. Test: a_child_dropped_while_the_history_load_is_incomplete_frees_nothing. Fails before the fix (ff-FF_M1.txt, and ff-r2-FF_R2_M1.txt "rescanned again: D"); passes at the head.
(2) Holds survive a quit mid-replay. Test: a_drop_cut_after_its_marks_keeps_its_held_coin_out_across_restarts. Fails before the fix (ff-FF_M2.txt); passes at the head.

THE REST OF THE FAIL-FIRST EVIDENCE (each fails without its fix, passes at the head):
- Two rivals: a_payment_two_rivals_beat_keeps_the_coin_a_locked_rival_spends_out (ff-two-rivals.txt).
- Round 2 review, M1: unknown inputs are written to the note so they're held across restarts (ff-r2-FF_R2_M1.txt).
- Round 2 review, M2: a failed read of the holds now fails the wallet open (an_open_that_cannot_read_a_drops_holds_fails, ff-r2-FF_R2_M2.txt). The holds are read once at open and kept in the hub, so there's no second read left to fail.
- Round 2 review, M4: cross-wallet (nothing_is_dropped_while_another_wallet_loaded_incomplete, ff-r2-M4.txt, 153 passed, 1 failed), and a wallet mid-open (nothing_is_dropped_while_another_wallet_is_opening, ff-r3-MIDOPEN.txt).
- Abandon keeps #746's rule while another wallet opens: abandoning_while_another_wallet_is_opening_is_not_refused_as_unreadable (ff-r4-ABANDON-MIDOPEN.txt).
- No test for: L5 (re-check under the write lock that the root is still beaten by the same winner) and L6 (marks deleted when key-wallet refuses). Both are lock races with no deterministic test. A failed history load now marks the wallet unread, also without a test: only a dead session or a panicked task reaches it.

FULL RUN at 36e886df5: `cargo test -p dw-engine --tests --no-fail-fast` (tests-full-7.txt, sha on line 1), exit 0. 29 targets, 0 failed. lib 3011 passed, 60 ignored; e0_05_bringup 13; session_lifecycle 13; r2_compat_offline 22; r1_tools_offline 16; restore_utxos_regtest 7; m4_dashpay_contract 9; mg_governance_contract 10; mn_masternodes_contract 8; e1_offline 8; dp1_03_names 8 (1 ignored); the rest 1–4 each; h06_perf 0 (1 ignored). clippy --workspace --all-targets -D warnings: clean. fmt: clean. No fail-first toggles remain (grep).
- Possible flake: e0_05_bringup start_spv_returns_at_once_and_spv_starts_after_the_bring_up failed once, in run 6 under load. Rerun alone it passed 13/13 twice, and it passed in run 7.

REVIEWS (claude-opus-5-5 high, independent):
- Round 1: REQUIRES CHANGES (M1, M2, M4, L5, L6). All fixed.
- Round 2: APPROVED WITH SUGGESTIONS (mid-open gap, second read, duplicate scan). All fixed.
- Re-check: APPROVED WITH SUGGESTIONS (Abandon's misleading "restore" refusal while a wallet opens; the failed-load gap). Both fixed in 36e886df5.

LEFTOVERS, not fixed (in the PR body):
- M3: observed_spent is pruned before the reconcile on a ChainLocked catch-up. The sweep path has no L5-style re-check, which is the same family. key-wallet's own sweep has already made that call.
- L7: no way to release holds. Filed as #749, which is now RC-T1 (visible way out). User path today: remove the wallet and restore it from its recovery phrase. Removing the wallet deletes its `dropped:` notes, and the restore rescans. Reset chain data does not clear the holds.
- L8: a `None` hold can replace a live payment's claim on a shared coin: if that payment is abandoned later in the same session, the coin stays held. Conservative (holds more, never frees), in session only.
- L9: an older build ignores `dropped:` (a downgrade releases the holds); and the abandon path's `debug_assert_eq!(abandoned, planned)` is not checked in release builds.
- L10: dropped payments show as "Abandoned".
- Case 5: parked for 1.0.x (PLAN-1.0.md row 5, which links #237 and MG's disclosure comment).

AFTER THE RULING: merge main (rerere off), re-run the full --tests, sample (one real reading under 30), push once, open the PR with this body, then "ready: #N at <sha>" at five green.
