# #749 plan (RC-T1, visible way out only; no release on heuristic)
- Surface: Home Dash wallet card only. home.core.heldUnfinished + "What can I do?" -> wallet.heldUnfinished.wayOut / wayOutSingle (MG copy verbatim; #691 placeholders {settings} {walletTab} {addWallet}).
- No pay.notSelectable.heldUnfinished.
- Restore sentence (MG final): "On {optionsTitle}, under {scanFrom}, choose The beginning." in both variants; render onboarding.options.title + .scanFrom; short "The beginning" has no key (birthDateMissing inlines it) -> literal, or add one shared key used by both; nothing else changes.
- Engine: read-only held total from dropped: notes + their outputs (NOT funds balance).
- Tests:
  (a) remove+restore clears dropped: notes; unspent held coin spendable after full rescan. If held state survives -> STOP, tell PO.
  (MG) held coin of value X shows X on card; spendable balance excludes it.
  (c) watch-only: confirm whether it can reach the drop BEFORE UI.
- Line on every network incl. mainnet; no "#NNN" in non-test source.
- Before push: diff + sha256, light/dark before/after shots, Visual + e2e home (Chromium+WebKit) at sha.
- ETA PR 11-26 18:00Z; heads-up by 06:00Z if slipping.

# L10 (MG final, 1.0 issue filed by PO; shared with lane 1 #739)
- activity.status.dropped label "Dropped"
- activity.status.dropped detail (released): "Dash Wallet dropped this payment because it can never confirm: it depends on a coin or payment that was spent or replaced elsewhere. Any of your own coins it used are back in your balance."
- activity.status.droppedHeld detail (held): "Dash Wallet dropped this payment because it can never confirm. It can't tell yet whether the coins it used were spent elsewhere, so it holds them back, and they're counted in the held amount on Home. To get back any that weren't spent, open What can I do? under that amount on Home."
- Conditions: variant from engine released/held flag; "back in balance" only once release applied; held row coins counted in #749 held amount; later release switches variant; "Abandoned" only for user abandons.

# held_unvalued lines (MG, final) — NEVER hide the card line
- held_unvalued == 0: home.core.heldUnfinished "{amount} held by an unfinished payment"
- some valued + held_unvalued > 0: home.core.heldUnfinishedAtLeast "At least {amount} held by an unfinished payment" ({amount} = valued only)
- all unvalued: home.core.heldUnfinishedUnvalued one "A coin held by an unfinished payment" / other "{count} coins held by an unfinished payment" ({count}=held_unvalued)
- all keep "What can I do?"; renderer test for 3 states; all 3 in light/dark shots
# watch-only way-out (MG final): wallet.heldUnfinished.wayOutWatchOnly / wayOutWatchOnlySingle (text in PO msg 11-24 ~19Z; {settings} ▸ {walletTab}, addWatchOnly interpolated). Engine choose variant by watch_only flag. Test: remove + re-add watch-only clears hold, coin back after scan (import birthHeight null -> 0).
