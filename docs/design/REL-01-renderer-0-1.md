# REL-01: the renderer pieces of 0.1

The 0.1 release plan (`docs/research/gap-2026-10-31/RELEASE-PLAN.md`, on `docs/gap-audit-release-plan-1031`) adds six renderer pieces. This note records what each one does and which command it waits for.

| # | Piece | Where | Notes |
|---|---|---|---|
| D1 | Network picker | `components/networkPicker.tsx`, `network/choice.ts`; Welcome (onboarding) and Settings ▸ General ▸ Network | Mainnet / Testnet. The choice is what the next start opens; the window keeps its network, so a different choice shows "Restart Dash Wallet to use …" (the backend's `NetworkChoiceView.restartNeeded`). Welcome disables Create and Restore until the choice is the window's network. A window started with `--network` (`forcedByFlag`) says so. |
| D2 | Export CSV | `history/exportCsv.ts`; Activity header | `export_history_csv` (already bound) over the filters and sort Activity shows, all pages, in the display unit and the local UTC offset; saved through `save_file_as` (purpose `csv`). A cancelled dialog says nothing. |
| D3 | Show Masternodes | `governance/queries.ts` `useMasternodesItem`, `app/App.tsx` | The setting (`UiSettings.showMasternodes`, off by default) alone shows the sidebar item, the governance status item and the route (Governance and Username voting are its tabs). A proposal in progress or a collateral-sized coin no longer shows it. A link to the route while it is hidden lands on Home. |
| D4 | CoinJoin "Experimental" | `ExperimentalBadge` in `components/basics.tsx` | Sidebar item, page title, Home card, Settings tab and card. |
| D5 | First-run notice | `screens/PreReleaseNotice.tsx`; `UiSettings.preReleaseNoticeDismissed` (dw-app `settings.rs`, serde-defaulted so older files read as not dismissed) | One screen over everything, dismissed once. The fixture starts past it; the `pre-release-notice` scenario shows it. |
| D6 | Home balance | `format/homeBalance.ts`, `screens/Overview.tsx` | Hero = Core total (mixed coins included) + Shielded + Platform addresses. Breakdown: Core (with "x pending" under it), Shielded, Platform (always shown), Immature when there is any. A pool whose balance is unknown shows "—" and is left out of the hero. The Platform figure is `platform_addresses().duffs`. |
| D7 | Move to Dash wallet | `platform/move.ts`, `screens/PlatformMoveSheet.tsx`; a card on Home while the Platform addresses hold something | Everything on the Platform addresses goes to the wallet's own Core receive address: the quote (`platform_move_to_core_quote`: credits taken, fee, Dash expected, address) → Confirm & move under the quote's `platformOp` grant → progress → outcome (`done`, or `pending` when Platform's answer said nothing: "do not move again") or a refusal by code. One flow per wallet; the sheet can close while the call runs and reopen to its result. |
| D8 | Platform rows in Activity | `history/platformRows.ts`, `screens/Transactions.tsx` | `platform_activity` rows (incoming, outgoing, move) join the List under "All" with no search text, by date, tagged "Platform"; they open no detail and are not in Table mode or the CSV. While Core has more pages, Platform rows older than the loaded Core rows wait for them. The footer counts them. |

## Commands the renderer is built against (`api/pending.ts`)

Lane E2's network choice and Platform addresses are in contract m4-dashpay-engine v40 (§2.12, §8, branch `dw/rel-platform-addr`); lane E1's shielded calls follow below. None is in `generated/commands.ts` of this branch yet; `IpcBackend` calls them by name through `callPending` (or `callRequired` for a save or a move of funds), which answers `null` while dw-app has no such command (Tauri's ACL refusal or "Command … not found", the fixture's `platform.not_implemented`), so a screen shows its "unavailable" state. The types are the generated bindings' shapes by hand (the codegen test refuses extra files in `generated/`). When E2's commits reach main, each adapter line in `api/tauri.ts` becomes `cmd.<name>`, its type an import from `generated/`, and its entry leaves `PENDING_COMMANDS`.

| Backend method | Command | Arguments | Answer |
|---|---|---|---|
| `networkChoice()` | `get_network_choice` | none | `NetworkChoiceView` |
| `setNetworkChoice(network)` | `set_network_choice` | `{ network }` | `NetworkChoiceView` |
| `platformAddresses(walletId)` | `platform_addresses` | `{ walletId }` | `PlatformAddressBalance` |
| `platformActivity(walletId)` | `platform_activity` | `{ walletId }` | `PlatformActivity[]` |
| `platformMoveToCoreQuote(walletId, amount)` | `platform_move_to_core_quote` | `{ walletId, amount }` | `MoveToCoreQuote` |
| `platformMoveToCore(walletId, amount, grant)` | `platform_move_to_core` | `{ walletId, amount, grant }` | `MoveToCoreOutcome` |
| `shieldedBalance(walletId)` | `shielded_balance` | `{ walletId }` | duffs (placeholder until E1's contract is read) |

`platform_addresses_sync` (the manual sync) is not used: the Platform queries sit under `["dashpay", …]`, which events.ts re-reads on every `platform` signal, `platformAddresses` included. Move-to-Core is the only amount `all`.

The fixture answers them from `World.pending` (`fixture/pending.ts`); a test sets a field to `null` to make the commands absent, and `platform.nextMove` to `pending` or `refused` for a move.
