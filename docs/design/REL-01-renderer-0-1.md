# REL-01: the renderer pieces of 0.1

The 0.1 release plan (`docs/research/gap-2026-10-31/RELEASE-PLAN.md`, on `docs/gap-audit-release-plan-1031`) adds six renderer pieces. This note records what each one does and which command it waits for.

| # | Piece | Where | Notes |
|---|---|---|---|
| D1 | Network picker | `components/networkPicker.tsx`, `network/choice.ts`; Welcome (onboarding) and Settings ▸ General ▸ Network | Mainnet / Testnet. The choice is what the next start opens; the window keeps its network, so a different choice shows "Restart Dash Wallet to use …". Welcome disables Create and Restore until the choice is the window's network. A window started with `--network regtest` or `devnet-<name>` says so. |
| D2 | Export CSV | `history/exportCsv.ts`; Activity header | `export_history_csv` (already bound) over the filters and sort Activity shows, all pages, in the display unit and the local UTC offset; saved through `save_file_as` (purpose `csv`). A cancelled dialog says nothing. |
| D3 | Show Masternodes | `governance/queries.ts` `useMasternodesItem`, `app/App.tsx` | The setting (`UiSettings.showMasternodes`, off by default) alone shows the sidebar item, the governance status item and the route (Governance and Username voting are its tabs). A proposal in progress or a collateral-sized coin no longer shows it. A link to the route while it is hidden lands on Home. |
| D4 | CoinJoin "Experimental" | `ExperimentalBadge` in `components/basics.tsx` | Sidebar item, page title, Home card, Settings tab and card. |
| D5 | First-run notice | `screens/PreReleaseNotice.tsx`; `UiSettings.preReleaseNoticeDismissed` (dw-app `settings.rs`, serde-defaulted so older files read as not dismissed) | One screen over everything, dismissed once. The fixture starts past it; the `pre-release-notice` scenario shows it. |
| D6 | Home balance | `format/homeBalance.ts`, `screens/Overview.tsx` | Hero = Core total (mixed coins included) + Shielded + Platform addresses. Breakdown: Core (with "x pending" under it), Shielded, Platform (always shown), Immature when there is any. A pool whose balance is unknown shows "—" and is left out of the hero. |

## Commands the renderer waits for (`api/pending.ts`)

Not in `generated/commands.ts` yet; `IpcBackend` calls them by name through `callPending`, which answers `null` while dw-app has no such command (Tauri's "Command … not found", the fixture's `platform.not_implemented`). When lane E1 or E2 generates one, the adapter line in `api/tauri.ts` becomes `cmd.<name>` and its entry leaves `PENDING_COMMANDS`.

| Backend method | Command | Arguments | Answer |
|---|---|---|---|
| `networkChoice()` | `get_network_choice` | none | `"mainnet" \| "testnet"` |
| `setNetworkChoice(choice)` | `set_network_choice` | `{ choice }` | `null` |
| `shieldedBalance(walletId)` | `shielded_balance` | `{ walletId }` | duffs |
| `platformBalance(walletId)` | `platform_address_balance` | `{ walletId }` | duffs |

The names and argument shapes of the last two are placeholders until E1 and E2 publish their contracts.

The fixture answers them from `World.pending` (`fixture/pending.ts`); a test sets a field to `null` to make the command absent.
