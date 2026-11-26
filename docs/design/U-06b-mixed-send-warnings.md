# U-06b: mixed-send warnings on the Pay review step

When I pay from **Fully mixed funds only (CoinJoin)**, the Confirm payment sheet tells me what the payment does to my
privacy before I press Send, as Dash Core does (QT-051, QT-059; RELEASE-PLAN 0.3 "mixed-send warnings (≥10 inputs, no
change)").

## What Dash Core does (reference)

dash-qt's send confirmation (`src/qt/sendcoinsdialog.cpp`, CoinJoin mode only) adds three lines and nothing else:

1. under the fee: "(CoinJoin transactions have higher fees usually due to no change output being allowed)";
2. "This transaction will consume %n input(s)";
3. at 10 inputs or more, in the warning colour: "Warning: Using CoinJoin with 10 or more inputs can harm your privacy and
   is not recommended", with a "Click to learn more" link to the Dash docs.

None of them blocks: Yes stays enabled. The iOS and Android wallets have no such warnings (their CoinJoin spend is the
"move mixed coins" sweep, CJ-13/CJ-16), so there is no mobile copy to follow.

## D-rows

| # | Decision | Why |
|---|----------|-----|
| D1 | The notices show on the Confirm payment sheet (`ConfirmSheet.tsx`) of `Pay ▸ To an address` and `Pay ▸ To a contact`, and only when Using is **Fully mixed funds only** (`coins === "mixed"`, `draft_set_source` `coinJoin`). They also show on the Unsigned transaction sheet (PSBT), since the payment is the same once signed. | dash-qt shows them only in CoinJoin mode, at confirmation. |
| D2 | Coins line, always in mixed mode, first of the three (then the D3 warning, then the D4 note, as dash-qt orders them): "This payment spends {count} mixed coin(s)." (plural forms), from `TxEstimate.inputCount`. | dash-qt line 2, in the app's words ("coin", as the coin-control UI says, not "input"). |
| D3 | Privacy warning when `inputCount >= 10` (`MIXED_INPUT_WARNING = 10`, dash-qt's constant): "Spending 10 or more mixed coins in one payment can link them together and weaken your privacy. Consider paying a smaller amount." Styled and announced like the high-fee warning (`fee-warning`, `role="alert"`). | dash-qt line 3. Many mixed inputs in one transaction are a common-input-ownership signal that undoes part of the mixing. |
| D4 | Left-over note, in mixed mode when something is left over: "Mixed payments have no change: the {amount} left over after the payment goes to the network fee.", `{amount}` the estimate's `leftOver`, formatted like the sheet's other amounts. A zero left-over shows no note. The Network fee line already holds the left-over (`fee = rate × size + leftOver`). | dash-qt line 1, with the figure (MG). The engine drops the change into the fee (`test_qt_072_coinjoin_page_pays_all_change_as_fee`, `TxEstimate.change === null`); the note says why the fee is high and by how much. A left-over large next to the payment also trips the existing high-fee warning (fee ≥ 10 % of the payment); D10 covers one that is large in absolute terms. |
| D5 | Inform only. Send stays enabled, with no countdown and no extra confirmation. | dash-qt is informational; the payment is the user's call. |
| D6 | No "learn more" link. | The renderer has no general external-link opener (only the explorer for a txid); a link is a follow-up if one lands. |
| D7 | Not warned: coin control (`Selected coins`) mixing mixed and unmixed coins, and a mixed send below 10 coins. | dash-qt warns about neither. The mixed source cannot mix the two: it spends only fully mixed coins (`test_qt_051_fully_mixed_candidates_need_final_free_coins`). |
| D8 | One engine read: `TxEstimate.left_over` (dw-engine and dw-app), `Plan::left_over` = `fee − fee_for(rate, size)`, which is a dust remainder or a CoinJoin payment's change plus its output's fee, and 0 otherwise. The fixture computes it the same way. dw-ffi's estimate is unchanged (the Swift app shows no such note). | `fee` alone does not say how much of it is left-over. The size after the change is dropped is the plan's no-change size, so the figure is exact for a payment of fewer than 251 recipients (Pay sends to one). With subtract fee and change, the payee already paid the dropped change output's fee (34 duffs at 1 duff/byte, as before this change), so the left-over includes it. |
| D10 | From a left-over of 0.01 DASH (`MIXED_LEFT_OVER_WARNING = 1_000_000` duffs), unless the high-fee warning shows, a warning takes D4's note's place, styled like D3: "The {amount} left over after this payment goes to the network fee, because mixed payments have no change. To lose less, raise the amount so more of it goes to the recipient." With the high-fee warning, D4's note shows instead. Same with subtract fee. | The 10 % warning misses a large left-over on a large payment (0.09 DASH on 50). The left-over is capped at MAX_TX_FEE (0.1 DASH). Copy and threshold: MG. |
| D9 | The fixture needs no new scenario: the specs seed twelve fully mixed 0.100001 coins into `dashpay-registered` or `funded`, so an 11-coin payment shows every notice. | The warning needs fixture coverage. |

## Copy (catalog `pay.confirm.mixed.*`)

- `coins`: one "This payment spends {count} mixed coin." / other "This payment spends {count} mixed coins."
- `manyCoins`: "Spending 10 or more mixed coins in one payment can link them together and weaken your privacy. Consider paying a smaller amount."
- `noChange`: "Mixed payments have no change: the {amount} left over after the payment goes to the network fee."
- `largeLeftOver`: "The {amount} left over after this payment goes to the network fee, because mixed payments have no change. To lose less, raise the amount so more of it goes to the recipient."

## Tests

- Rust: `the_left_over_is_the_fee_beyond_the_rate` (dw-engine `send/plan.rs`): a mixed payment's left-over is its change
  plus the change output's fee, and the fee is the rate's fee plus it; a normal or exact payment leaves none.
- vitest: `mixedSendNotes` at 9, 10 and 11 coins, a left-over only when non-zero, nothing for `any` and `selected`; the
  fixture's mixed estimate's fee holds its left-over.
- Playwright (fixture, light and dark, Chromium and WebKit), `e2e/mixed-send.spec.ts`: an 11-coin mixed payment to an
  address, to a contact and as an unsigned transaction shows all three and a fee line that holds the left-over; a
  1-coin subtract-fee payment shows the count only; an `Any` payment shows none.
- Pseudo-locale and catalog tests cover the new keys.
