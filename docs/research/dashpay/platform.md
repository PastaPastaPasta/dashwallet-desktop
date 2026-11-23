# Research: Platform / DashPay capabilities for the desktop engine

Status: research, read-only. Written 2026-10-07 (agent run). Nothing in the repo was changed except this file.

Path shorthands used below:

| Short | Absolute path |
|---|---|
| `P/` | `/Users/pasta/workspace/dashwallet-desktop-deps/platform/packages/` (pin `bc321362b9`) |
| `PW/` | `P/rs-platform-wallet/src/` |
| `PWS/` | `P/rs-platform-wallet-storage/src/` |
| `PWF/` | `P/rs-platform-wallet-ffi/src/` (the iOS/Android C ABI; **not** linked by desktop) |
| `KW/` | `~/.cargo/git/checkouts/rust-dashcore-c6b13647c01f74b9/e4208c9/key-wallet/src/` |
| `IOS/` | `/Users/pasta/workspace/dashwallet-ios/DashWallet/Sources/` (develop `3df12fc89`, 2026-10-06) |
| `AND/` | `/Users/pasta/workspace/dashwallet-desktop-deps/dash-wallet-android/` (master `a8de1f0`, 2026-10-07) |
| `DWD/` | `/Users/pasta/workspace/dashwallet-desktop/` |

Related earlier research (not repeated here): `DWD/docs/research/01-sdk-stack.md` (FFI layering, persistence model, Swift portability), `DWD/docs/research/03-ios-features.md` (iOS feature inventory), `DWD/docs/design/DESIGN-opus.md` §M4 / WS-07 (Platform engine plan, U5).

---

## 0. TL;DR

1. **The library already does almost all of DashPay.** At the pin, `platform-wallet` (rs-platform-wallet) implements identity discovery/registration/top-up/withdraw/transfer, asset-lock funding with InstantSend proof and ChainLock fallback plus resume and recovery, DPNS register/resolve/search, contested-name vote state, the username marketplace, DIP-15 contact requests (send/accept/ignore/auto-accept QR), DIP-14 256-bit contact derivation, contact payments with incoming/sent reconciliation, profiles with avatar hash and dHash, encrypted contactInfo (alias/note/hidden), invitations (create/claim/parse), Platform (DIP-17) addresses with BLAST sync, tokens, and the shielded pool. It also ships the ordered bring-up `PlatformWalletManager::start_wallet_subsystems` (`PW/manager/startup.rs:515`) and four background sync loops (identity/token, DashPay, DPNS, Platform address).
2. **The desktop engine uses none of it yet.** `dw-engine` builds the SDK with the trusted quorum provider (`DWD/rust/crates/dw-engine/src/session.rs:601`, `context.rs`) and a `PlatformWalletManager<WalletStore>` over `SqlitePersister` (`session.rs:283`), but no code calls `wallet.identity()`, `dashpay()`, or any sync manager (grep of `dw-engine/src` and `dw-ffi/src`). `dw-ffi` has no Platform module. Wallets are created with `WalletAccountCreationOptions::Default` (`keys.rs:367`), so the identity registration/top-up/invitation and Platform payment accounts already exist and are persisted.
3. **What the desktop must write is glue, not protocol.** The production signers for Platform live in the FFI crates that desktop does not link (`P/rs-sdk-ffi/src/mnemonic_resolver_core_signer.rs`, `PWF/dashpay.rs:740` `ResolverContactCryptoProvider`). Desktop needs three vault-backed adapters: (a) `dpp::identity::signer::Signer<IdentityPublicKey>` for identity keys, (b) `platform_wallet::ContactCryptoProvider` (ECDH, account reference, contactInfo seal/open, auto-accept and invitation key export), (c) a `ScanKeyResolver` returning the master xprv for discovery. `dw_vault::VaultSigner` already covers the Core/asset-lock `ExtendedPubKeySigner + Signer`. All the crypto is in `P/rs-platform-encryption` and the test-only `SeedCryptoProvider` (`PW/wallet/identity/network/contact_requests.rs:172-400`) is a working template. About 2–3 days.
4. **SqlitePersister has real gaps for DashPay** (they exist at the pin, at v5.0-dev head and at v5.1-dev): it does not attest `WALLET_RESTORE` (`PWS/sqlite/persister.rs:1306-1333`), and `load()` does not read back `invitations`, `pending_contact_crypto`, `token_balances` or the DashPay overlay tables (`LOAD_UNIMPLEMENTED`, `persister.rs:44`). Consequences: **`create_invitation` is refused** (it requires `INVITATION_CREATION` = `…|WALLET_RESTORE`, `PW/changeset/persistence_capabilities.rs:118`, checked at `PW/wallet/identity/network/invitation.rs:258`); the deferred contact-crypto queue is dropped on restart (it heals on the next sync sweep); the asset-lock "consumption unknown" record fails (`ASSET_LOCK_RECONCILIATION`, `PW/wallet/asset_lock/sync/tracking.rs:330`); DPNS marketplace state is session-scoped. DESIGN-opus U5 covers only token balances and the overlay. Invitation creation needs an upstream patch (or stays out, as on iOS).
5. **Pin vs default branch.** dashpay/platform's default branch is `v5.0-dev` (GitHub API). The pin is a strict ancestor: 35 commits behind head `bc41f1bc23` (2026-10-07). There is no API break in `platform-wallet`/`platform-wallet-storage` for us. iOS depends on two things that landed after the pin: #5206 (typed `InsufficientIdentityCredits` on withdrawal, used by `IOS/UI/Payments/InternalTransfer/IdentityWithdrawViewModel.swift:181-193`) and #4978 (DPNS paging plus "keep the chosen DPNS name across wallet sync"). Security: at the pin a PV13 client still **accepts GroveDB V0 proof envelopes** (`P/rs-platform-version/src/version/system_limits/v1..v3.rs: minimum_grovedb_proof_envelope_version: 0`); head refuses them for every protocol version (#5294, `rs-drive/src/verify/grovedb_proof_envelope.rs` on head). Testnet and mainnet run PV13 (drive 4.1.0-rc.2 / 4.1.0). **Recommendation: bump to v5.0-dev head once, at the start of the DashPay work** (about 0.5–1 day plus a full rebuild and regression run). Do not move to v5.1-dev yet (it lags v5.0-dev by 18 commits and carries PV15); revisit when iOS ships the v5.1 features (any-balance contact pay #4623, idempotent invitation claim #4997, signing-key selection #4764).
6. **Testing.** Testnet + `faucet.thepasta.org` is the main path. The faucet is live (1 tDASH per request, 3/h, CAP) and has an `asset-lock-proof` endpoint that can fund an identity without L1 funds. It also hands out **mainnet** DashPay invitations (0.03 DASH) for a real claim test. A local dashmate network works (it has a `quorum_list` service for the trusted provider) but is heavy: run it on agentbox or the Studio, not the laptop. Regtest (`DWD/regtest`) covers only the Core half (asset-lock transaction, IS/CL proof waits).

---

## 1. Revisions examined

| Repo | Revision | Notes |
|---|---|---|
| dashpay/platform (pin) | `bc321362b9ff9c8ac24244f9d4b09ec3de7a5938` (2026-10-04, v5.0-dev) | `DWD/rust/Cargo.toml` workspace deps; checkout at `/Users/pasta/workspace/dashwallet-desktop-deps/platform` (HEAD unchanged). |
| dashpay/platform default branch | `v5.0-dev` @ `bc41f1bc23` (2026-10-07 23:48) | `gh api repos/dashpay/platform --jq .default_branch` = `v5.0-dev`. Fetched over HTTPS into `refs/remotes/origin/v5.0-dev` (the SSH remote failed: publickey). |
| dashpay/platform v5.1-dev | `6499c680c6` (2026-10-06) | 54 ahead / 18 behind v5.0-dev; the pin is an ancestor. |
| rust-dashcore | pin `e4208c90` → head uses `40268cc0` | 5 commits, all in `rpc-json/src/lib.rs` (Core v24 masternode RPC types). Desktop does not use `dashcore-rpc` in any crate. |
| dashwallet-ios | develop `3df12fc89` (2026-10-06) | `.github/workflows/release-dashpay-testflight.yml:15-18,212-218`: blank `platform_ref` = dashpay/platform default branch (`v5.0-dev`). |
| dash-wallet-android | master `a8de1f0` (2026-10-07) | Uses **dashj 22.0.5 + dashj-platform `dash-sdk` 4.0.1** (`AND/build.gradle:6-7`, `AND/wallet/build.gradle:16-23,518-522`), not rs-platform-wallet. It is a UX reference only. |
| Live networks (2026-10-08 02:10 UTC) | testnet: dapi/drive 4.1.0-rc.2, tenderdash 1.6.0, **protocol 13**; mainnet (evo1): 4.1.0, **protocol 13** | From `https://{testnet.,}platform-explorer.pshenmic.dev/status`. |

---

## 2. How the desktop engine gets DashPay

### 2.1 Today (what `dw-engine` already has)

| Piece | Where | State |
|---|---|---|
| Platform SDK (`dash_sdk::Sdk`) | `DWD/rust/crates/dw-engine/src/session.rs:601-626` `build_sdk` | Mainnet/testnet default DAPI list (`SdkBuilder::new_{mainnet,testnet}` → `dash_network_seeds::evo_seeds`, `P/rs-sdk/src/sdk.rs:125-140`); devnet/regtest need explicit `dapi_addresses`. No CA-certificate option (needed for dashmate's self-signed gateway, §8.2). No explicit protocol version: the SDK seeds PV13 on mainnet/testnet/regtest and PV14 on devnet, then ratchets up (`P/rs-sdk/src/sdk.rs:60-77`). |
| Proof trust | `dw-engine/src/context.rs` `LazyTrustedContext` | `TrustedHttpContextProvider` (`quorums.{mainnet,testnet}.networks.dash.org`, devnet `quorums.<name>.networks.dash.org`), built lazily because the constructor does a blocking DNS check (`P/rs-sdk-trusted-context-provider/src/provider.rs:150-200`). Quorum prefetch at open. Custom URL must be https on mainnet/testnet; plain http allowed on devnet/regtest (`provider.rs:175-191`). |
| Manager | `session.rs:283-287` `PlatformWalletManager::new(sdk, WalletStore, SessionHub)` then `load_from_persistor()` | Generic over `WalletStore` (`dw-engine/src/store.rs`), a pass-through wrapper over `SqlitePersister` that hides closed wallets. It forwards `persistence_capabilities()` unchanged. |
| Wallet state DB | `<data>/<network>/wallet.sqlite` (`session.rs:30`) | `SqlitePersister` stores every Platform changeset the library emits (identities, keys, contacts, asset locks, invitations, DPNS states, profiles…) even though nothing produces them yet. |
| Accounts | `keys.rs:367` `WalletAccountCreationOptions::Default` | Creates BIP44/BIP32/CoinJoin 0 plus all special accounts: identity registration, top-up, invitation, PlatformPayment (`KW/wallet/initialization.rs:44-50`, `KW/wallet/helper.rs:134-170`). DashPay contact accounts are added on demand by platform-wallet. History already walks `dashpay_receival_accounts` (`history_ops.rs:79`, `coins.rs:189`). |
| Signing | `DWD/rust/crates/dw-vault/src/signer.rs` `VaultSigner` | key_wallet `ExtendedPubKeySigner + Signer` over the vault seed; scopes `Full`, `CoinJoinOnly`, `CoinJoinFunding` (`signer.rs:25-37`). It already fits the asset-lock signer `AS` and the Core signer of `send_payment`. There is no dpp identity signer and no `ContactCryptoProvider`. |
| Deep links | `dw-uri/src/deeplink.rs:36,145,224-300` | Parses `dashpay://user?id=&username=` and invitation links (`dashpay://invite?…`, `https://invitations.dashpay.io/applink?…`). Nothing consumes them. |
| Sync loops | — | None started. `DashPaySyncManager` and its siblings are not auto-started (`PW/manager/dashpay_sync.rs:39-41`). |

### 2.2 Target shape (same pattern as iOS, minus the C ABI)

```
NetworkSession (dw-engine)
  ├─ PlatformWalletManager<WalletStore>          (exists)
  ├─ platform/ (new module, WS-07)
  │    signers.rs   VaultIdentitySigner      : dpp Signer<IdentityPublicKey>   (identity keys, DIP-13 m/9'/c'/5'/0'/0'/i'/k')
  │                 VaultContactCrypto       : platform_wallet::ContactCryptoProvider
  │                 scan_key()               : ScanKeyResolver (master xprv, on demand)
  │    startup.rs   start_wallet_subsystems(wallet, scan_key, contact_crypto, identity_signer)  BEFORE Core SPV start/rescan
  │                 then start identity_sync / dashpay_sync / dpns_sync / platform_address_sync; quiesce on close
  │    identity.rs, dpns.rs, contacts.rs, profile.rs, payments.rs, invitations.rs, credits.rs, marketplace.rs …
  └─ events: Platform domain events → SessionHub → UniFFI callback (DESIGN-opus "PlatformDomainEvent")
dw-ffi/src/api/{identity,dpns,dashpay,invitation,platform_address,…}.rs   (records only)
DashKit → WalletRuntime adapters → WalletFeatures @Observable VMs → MacUI / CrossUI
```

Key-material rule (same as iOS, `PW/manager/startup.rs:26-36`): the providers are built for one call and dropped. Background loops hold no signer. Signer-needing work (contact-account builds, auto-accept) is queued and drained when a signer is present: after unlock, on any DashPay action, and in `start_wallet_subsystems`.

Ordering rule (`PW/manager/startup.rs:1-24`): contact accounts must exist before the compact-filter scan passes their funding heights, or contact payments never appear. So the desktop runtime has to run `start_wallet_subsystems` (20 s budget, `DEFAULT_STARTUP_BUDGET`) before starting SPV. It must still start SPV regardless of the outcome; the outcome only changes what to show. Whatever is late is fixed by `reconcile_dashpay_rescan` (`PW/wallet/identity/network/payments.rs:103`).

Signer scope: add `SignerScope::Platform` (identity, DashPay and asset-lock paths: `m/9'/coin'/{5',15',16'}/…`) next to the CoinJoin scopes, so a DashPay drain cannot sign BIP44 spends. The vault must allow DIP-14 256-bit child numbers (`ChildNumber::Normal256`, `PW/wallet/identity/crypto/dip14.rs:1-30,96-130`); `VaultSigner::derive` uses key_wallet's `derive_priv`, which supports them (UNVERIFIED with the vault's path checks).

---

## 3. Capability table

Legend. **Lib@pin** = support in `platform-wallet`/`dash-sdk` at `bc321362b9`. **Lib@head** = change at `v5.0-dev` head `bc41f1bc23`. **Desktop gap** = what is missing in `dw-engine`/`dw-ffi`/Swift. **Est.** = rough agent-days: engine + FFI / Swift VM + UI (macOS + Cross), assuming the §2.2 glue exists. iOS parity IDs refer to `DWD/docs/parity.md`.

| # | Feature (parity id) | Lib@pin (file:line) | Lib@head | Desktop gap | Est. engine / UI |
|---|---|---|---|---|---|
| 0 | **Signer glue**: identity signer, contact crypto, scan key | Traits: `rs-dpp/src/identity/signer.rs`; `PW/wallet/identity/network/contact_requests.rs:36-170` (`ContactCryptoProvider`); `PW/manager/startup.rs:97-150` (`ScanKeyResolver`). Production impls only in FFI crates (`P/rs-sdk-ffi/src/mnemonic_resolver_core_signer.rs:372-584,670`; `PWF/dashpay.rs:740-871`). Template: test `SeedCryptoProvider` (`contact_requests.rs:172-400`). | same | All three adapters over `dw_vault`; add `platform-encryption` (path `P/rs-platform-encryption`) to the workspace deps; new `SignerScope`. | 2–3 / 0 |
| 1 | **Bring-up and background sync** (IOS-114) | `start_wallet_subsystems` `PW/manager/startup.rs:515`; `identity_sync()` / `dashpay_sync()` (15 s default, 8 MB stack) / `dpns_sync()` / `platform_address_sync()` accessors `PW/manager/accessors.rs:569-625`; `start`/`stop`/`quiesce`/`sync_now` in `PW/manager/{identity_sync,dashpay_sync,dpns_sync,platform_address_sync}.rs`. | same | Runtime order host → **subsystems** → SPV → loops; quiesce before `shutdown()`; Sync Info screen. | 2–3 / 1–2 |
| 2 | **Identity discovery / restore** | `discover`, `discover_from_master` `PW/wallet/identity/network/discovery.rs:175,225`; `IDENTITY_GAP_LIMIT = 5`, master key index 0 (`identity_handle.rs:52,57`); `load_identity_by_index[_from_master]` `loading.rs:119,147`; `load_identity_by_dpns_name` `loading.rs:478`; scan verdict persisted (`identity_scan_state`, V017). | DPNS names fetched with paging (`get_all_dpns_usernames_by_identity`) and merged as one snapshot (#4978). | Runs inside item 1; "Find identity" action (IOS-071). | 1 / 1 |
| 3 | **Identity registration funded from Core** (IOS-067) | `register_identity_with_funding` `PW/wallet/identity/network/registration.rs:121` (asset lock → IS proof, 300 s IS timeout → ChainLock fallback with the same outpoint, 180 s); `AssetLockFunding::{FromWalletBalance, DrainAccountBalance, FromExistingAssetLock}` `PW/wallet/asset_lock/orchestration.rs:162-260`; resume/recover `PW/wallet/asset_lock/sync/{tracking,recovery,reconstruction}.rs`; funding pooled over BIP44 + BIP32 + DashPay receiving, never CoinJoin (`orchestration.rs:174-185`). | same | The key-set policy is app-side on iOS: 4 base keys (AUTH/MASTER, AUTH/CRITICAL, AUTH/HIGH, TRANSFER/CRITICAL) + ECDSA ENCRYPTION and DECRYPTION (MEDIUM, bound to DashPay `contactRequest`), see `IOS/Infrastructure/SwiftDashSDK/Identity/DWDashPayIdentityKeys.swift`. Port to Rust. Progress events per phase (iOS polls `PersistentAssetLock` every 0.5 s); resumable registration (IOS-033, `AssetLockRecoveryService.swift`). | 4–6 / 3–4 |
| 4 | Registration from Platform addresses / shielded pool | `PlatformWallet::register_from_addresses` `PW/wallet/platform_wallet.rs:552`; `shielded_identity_create_from_pool` `:1571` (feature `shielded`). | same | Addresses: after item 13. Shielded: after item 14. | 1 / 1 (+shielded) |
| 5 | **Top-up / withdraw / transfer credits** (IOS-069, 070) | `top_up_identity_with_funding` `registration.rs:397`; `top_up_identity` `top_up.rs:44`; `withdraw_credits_with_{external_,}signer` `withdrawal.rs:73,151`; `transfer_credits_with_{external_,}signer` `transfer.rs:76,151`; `transfer_credits_to_addresses_with_external_signer` `transfer_to_addresses.rs:79`; `top_up_from_addresses` `platform_wallet.rs:519`; `refresh_identity_balance` `balance.rs:20`. | **#5206**: Platform's balance refusal on withdraw/transfer becomes `PlatformWalletError::InsufficientIdentityCredits` (`PW/error.rs`, +124 lines). iOS depends on it. | Fee reserve logic (iOS `IdentityWithdrawViewModel`), Core fee-rate picker. | 2–3 / 2 |
| 6 | Identity update (add keys, disable) | `update_identity_with_{external_,}signer` `update.rs:89,276`; key limits `key_limits.rs:37`. | same | "Enable DashPay" lazy ENC/DEC key upgrade (`IOS/…/Identity/DWIdentityKeyUpgrader.swift`, `SwiftDashSDKContactsService.swift:808-935`). | 1 / 1 |
| 7 | **DPNS register / availability / resolve / search** (IOS-066) | `register_name_with_{signer,external_signer}` `PW/wallet/identity/network/dpns.rs:140,186` (preorder + domain); `resolve_name` `:308`; `search_names` `:503`; `sync_dpns_names` `:338`; SDK `register_dpns_name`, `is_dpns_name_available`, `resolve_dpns_name`, `search_dpns_names` (`P/rs-sdk/src/platform/dpns_usernames/{mod,queries}.rs`). | #4978: paging up to a page bound, `DpnsFetch::{Complete,Partial}`, `apply_fetched_dpns_names` (watched identities: replace; owned: merge). Fixes "chosen (main) name lost after sync". | Label validation/normalisation (homograph-safe, iOS `dpnsNormalizeLabel`), debounced availability, contested detection. | 3–4 / 3 |
| 8 | **Contested names: vote state for the user's own request** (IOS-068) | `contest_vote_state` `dpns.rs:452` (`ContestVoteState`, `ContestContender`, `ContestWinner`); `sync_contested_dpns_names` `:406`; SDK `get_contested_dpns_vote_state`, `get_current_dpns_contests`, `get_non_resolved_dpns_contests_for_identity` (`P/rs-sdk/src/platform/dpns_usernames/contested_queries.rs:62-432`). | head #5321 fixes contested poll end dates (Drive side, PV14). | Status screen (contenders, tallies, abstain/lock, deadline) and a temporary username. **Masternode voting (IOS-079, `IOS/…/Voting/*`) is out of desktop scope** (CLAUDE.md product scope; wallets cannot vote). | 1–2 / 2 |
| 9 | **Contacts: send / accept / ignore / sync** (IOS-072…075) | `send_contact_request_with_external_signer` `contact_requests.rs:409`; `accept_contact_request_with_external_signer` `:3403`; `sync_contact_requests[_reporting]` `:1358,1374`; `sent_contact_requests` `:3751`; `ignore_contact_sender` / `unignore_contact_sender` `:3882,3933`; `established_contacts` `contacts.rs:116`; DIP-15 validation `PW/wallet/identity/crypto/validation.rs:191`; seed-binding gate `seed_binding.rs:158-600`; deferred crypto queue `pending_contact_crypto_count` / `drainable_…` `contact_requests.rs:2056,2076`. | same | Read model (established / incoming / outgoing / hidden) from `ManagedIdentity` + persisted contacts; notifications (bell, unread); username hints. iOS service: `IOS/Infrastructure/SwiftDashSDK/Contacts/SwiftDashSDKContactsService.swift` (1.2k lines). | 4–5 / 5–6 |
| 10 | QR auto-accept (DIP-15 `dapk`) — "My QR" / add-by-QR (IOS-073) | `build_auto_accept_qr` `contact_requests.rs:3120`; `send_contact_request_from_qr` `:768`; `drain_auto_accepts_verified` `seed_binding.rs:451`; URI codec `PW/wallet/identity/crypto/auto_accept.rs:88-360`. | same | iOS does **not** call these (no hits in `IOS/`); iOS "My QR" is a plain `dashpay://user` link. Optional extra. | 1 / 1 |
| 11 | **Pay a contact (DIP-15)** (IOS-050) | `send_payment` `PW/wallet/identity/network/payments.rs:1080` (drains this contact's deferred build first; returns txid, `PaymentEntry`, exact fee); incoming matching via `DashPayPaymentHandler` (`payment_handler.rs`) and `match_incoming_dashpay_address` `contacts.rs:282`; `reconcile_incoming_payments` / `reconcile_sent_payments[_from_tx_history]` / `reconcile_dashpay_rescan` `payments.rs:37,241,716,103`; contact address gap `DEFAULT_CONTACT_GAP_LIMIT = 10` (`crypto/dip14.rs:260`). | same. **v5.1-dev only**: #4623 "reserve DashPay payout addresses without Core funding" (any-balance contact pay; the matching iOS PR waits on v5.1). | Send-flow route "to contact" with the per-contact unknown-outcome lock (iOS `WalletSendService.swift`); history attribution (contact avatar/name on rows, IOS-029). | 2–3 / 3 |
| 12 | **Profile: create / update / sync, avatar** (IOS-076) | `create_profile_with_external_signer` / `update_profile_with_external_signer` `profile.rs:138,269`; `sync_profiles` `:52`; `sync_contact_profiles` `:560` (negative cache, `ContactProfileEntry`); `ProfileUpdate { display_name ≤25, public_message ≤140, avatar_url ≤2048, avatar_bytes }` computes SHA-256 + 64-bit dHash (`PW/wallet/identity/types/dashpay/profile.rs`, `image` crate png/jpeg/gif). | same | Avatar bytes must be downloaded app-side. Upload via Imgur (§10) or a Gravatar URL. | 2 / 3–4 |
| 13 | contactInfo: alias / note / hide (encrypted) (IOS-074) | `set_contact_info_with_external_signer` `contact_info.rs:504`; `sync_contact_infos` `:290`; crypto `PW/wallet/identity/crypto/contact_info.rs:92-300`. | same | Edit sheet. | 1–2 / 1 |
| 14 | **Claim an invitation** (IOS-077) | `claim_invitation` `PW/wallet/identity/network/invitation.rs:481` (voucher-key asset-lock signature, IS→fetch retry); `invitation_prospective_identity_id` `:424`; parse `crypto/invitation.rs:406`; link encode `:319` (`dashpay://invite?…`, carries the WIF; treat as a secret). | **v5.1-dev only**: #4997 claim status for the invitee and idempotent claim (`platform_wallet_invitation_claim_status`). | Paste/scan/link entry, inviter preview, "already claimed" check (v5.1), optional contact request back. `dw-uri` already normalises the links. | 2–3 / 2 |
| 15 | **Create / reclaim invitations** (IOS-078; Android has it, iOS removed it; DESIGN D11 says IN) | `create_invitation` `invitation.rs:220`; limits `MIN_INVITATION_DUFFS 300_000`, `MAX_INVITATION_DUFFS 26_000_000`, TTL 24 h (`:48-80`). | same | **Blocked with SqlitePersister**: needs `WALLET_RESTORE` (`invitation.rs:258`). Needs the upstream storage patch (§5.2) or a carried `[patch]`. Inviter-side list: `invitations` table has `read_all` (`PWS/sqlite/schema/invitations.rs:95`) but `load()` does not rehydrate it. | 2 + 3–5 upstream / 2 |
| 16 | **Platform (DIP-17) addresses, BLAST sync** (IOS-064, 045 routes) | `PW/wallet/platform_addresses/{wallet,sync,transfer,withdrawal,fund_from_asset_lock,provider}.rs`; `PlatformAddressSyncManager` `PW/manager/platform_address_sync.rs`; `reset_platform_address_sync_state` `PW/manager/mod.rs:845`. | same | Balance in hero (advanced mode), receive toggle, transfers. iOS: `PlatformAddressSyncCoordinator.swift` (2.3k), `PlatformSendExecutor.swift`. | 3–5 / 3 |
| 17 | **Username marketplace** (IOS-084) | `PW/wallet/identity/network/dpns_marketplace.rs`: `search_dpns_names_with_state` `:646`, `dpns_name_state` `:689`, `set_dpns_name_price` `:1078`, `delist` `:1136`, `transfer` `:1194`, `purchase` `:1293`, `dpns_name_history` `:1398`, `sync_dpns_marketplace` `:1525`; `DpnsSyncManager`. | #4978 adds about 500 lines (departure classification and "chosen name" retention). Bump first. | Note: `dpns_name_states` are session-scoped with SqlitePersister (no load reader, `PW/changeset/traits.rs:495-510`). The first sync after a restart cannot classify departures; mirror them in `dw-appdb`. | 4–6 / 4 |
| 18 | Tokens; DashConnect token purchase approval (IOS-085, 086) | `PW/wallet/identity/network/tokens/*.rs` (mint/burn/transfer/freeze/…/purchase/set_price/claim); `IdentitySyncManager` watches token balances; `PW/wallet/tokens/group_queries.rs`. | head #5324/#5325 burn/payment-policy fixes (consensus PV14). | iOS uses only `tokenPurchase` (DashConnect, scheme `dashid`). Defer. Token balances are not reloaded by SqlitePersister (U5). | 2 / 2 (defer) |
| 19 | **Shielded pool** (IOS-059…062) | `PW/wallet/shielded/*` behind feature `shielded` (halo2/orchard/grovedb-commitment-tree); `PlatformWallet::shielded_*` `platform_wallet.rs:781-1991`; `configure_shielded` `PW/manager/mod.rs:672`. | head #5014 (shield nullifiers, PV14). | Feature **off** in `DWD/rust/Cargo.toml` (DESIGN-opus G4). Turn on `platform-wallet/shielded` and `platform-wallet-storage/shielded`. Costs: halo2 build time and binary size, prover warm-up, a separate tree DB. iOS shows Shielded in the home hero by default. | 10–15 / 8–10 |
| 20 | Contract/document generic ops | `PW/wallet/identity/network/{contract,document}.rs` | same | Not needed for the wallet UI. | — |
| 21 | Masternode identities, evonode withdrawal, ProUpServTx | `PW/wallet/masternode_withdrawal.rs`, `PW/masternode/*` | head #5227 (Core v24 MN identities, PV14) | **Out of scope** (Dash Core keeps masternode tools). | — |

Rough total for DashPay parity without shielded, tokens or invitation creation: about **30–40 engine/FFI days and 30–40 UI days**, plus testnet suite work. Most UI cost is the iOS screen count (`IOS/UI/DashPay` is about 13k lines; the adapter `Identity/Contacts/Invitations` + marketplace is about 8.3k lines).

---

## 4. What iOS actually calls, per flow

Counts are from grepping `IOS/` for SDK method calls. Swift names map one-to-one to `PWF` functions and then to the `PW` methods in §3.

| Flow | iOS file(s) | Swift SDK calls | Rust (PW) behind it |
|---|---|---|---|
| Runtime bring-up | `IOS/Infrastructure/SwiftDashSDK/SwiftDashSDKHost.swift`, `SwiftDashSDKWalletRuntime.swift`, `Contacts/DashPayContactAddressReadiness.swift` | `loadFromPersistor`, `startWalletSubsystems`, `startDashPaySync`, `startDpnsSync`, `startPlatformAddressSync`, `isDashPaySyncRunning`, `dashPaySyncNow` | §3 rows 1–2 |
| Join DashPay / register | `Identity/DWIdentityRegistrationCoordinator.swift` (2.1k), `Identity/DWDashPayIdentityKeys.swift`, `Identity/ShieldedIdentityFundingReadiness.swift` | `prePersistIdentityKeysForRegistration`, `deriveIdentityAuthKeyAtSlot`, `registerIdentityWithFunding`, `resumeIdentityWithAssetLock`, `registerIdentityFromAddresses`, `shieldedIdentityCreateFromPool`, `claimInvitation`, `registerDpnsName`, `fetchContestVoteState` | rows 3, 4, 7, 8, 14 |
| Key upgrade | `Identity/DWIdentityKeyUpgrader.swift` | `identityGetKeys`, `updateIdentity` | row 6 |
| Contacts | `Contacts/SwiftDashSDKContactsService.swift` (:365 sync, :389 send, :460 accept, :502 ignore, :528 search, :680 contactInfo), `UI/DashPay/Contacts/SwiftUI/{AddContactScreen,ContactsScreen,ContactProfileSheet}.swift` | `dashPaySyncNow`, `sendContactRequest`, `acceptContactRequest`, `ignoreContactSender`, `searchDpnsNames`, `resolveDpnsName`, `setDashPayContactInfo` | rows 9, 13 |
| Pay contact | `Models/Transactions/WalletSendService.swift` | `sendDashPayPayment`, `refreshDashPayPayments` | row 11 |
| Profile | `Identity/DWProfileUpdateCoordinator.swift`, `Infrastructure/Networking/DWAvatarUploadClient.swift` | `createDashPayProfile`, `updateDashPayProfile`, `getDashPayProfile` | row 12 |
| Invitations (invitee only) | `Invitations/DWInvitationService.swift`, `Invitations/DWInvitationLinkNormalizer.swift`, `AppDelegate.m:254` (universal link `invitations.dashpay.io`) | `parseInvitation`, `invitationProspectiveIdentityId`, `claimInvitation`, then `sendContactRequest` | row 14 (no `createInvitation` call anywhere in `IOS/`) |
| Contest status | `Identity/DWContestedNameStatusService.swift`, `Identity/DWCurrentUserIdentityInfo.swift` | `fetchContestVoteState`, `syncContestedDpnsNames` | row 8 |
| Marketplace | `UsernameMarketplaceService.swift` | `searchDpnsMarketplace`, `myDpnsMarketplaceNames`, `dpnsMarketplaceNameState`, `purchaseDpnsName`, `setDpnsNamePrice`, `transferDpnsName`, `dpnsNameHistory`, `syncDpnsMarketplace` | row 17 |
| Credits | `UI/Payments/InternalTransfer/IdentityWithdrawViewModel.swift`, `UI/DashPay/Profile/SDKIdentityProfileSheet.swift` | `withdrawCredits`, `transferCreditsToAddresses`, `topUpIdentityWithFunding`, `topUpFromAddresses`, `refreshIdentityBalance` | row 5 |
| Masternode voting (not for desktop) | `Voting/{ContestedNamesService,MasternodeVoterRegistry,MasternodeVoteCaster}.swift` | `dash_sdk_contested_resource_cast_vote` path | out of scope |

Calls the SDK offers that iOS does **not** use: `build_auto_accept_qr` / `send_contact_request_from_qr`, `create_invitation`, `unignore_contact_sender`, `drain_pending_contact_crypto` (drained inside other calls).

---

## 5. Persistence coverage (`SqlitePersister`)

### 5.1 What is stored (pin = head = v5.1-dev; no storage changes after the pin)

`PWS/sqlite/persister.rs:2160-2245` applies every `PlatformWalletChangeSet` field (`PW/changeset/changeset.rs:2104-2200`): `wallet_metadata`, `account_registrations` (incl. DashPay contact accounts keyed by user/friend identity id), `provider_key_account_registrations`, `account_address_pools`, `pending_contact_crypto_{added,cleared}`, `core`, `shielded` (feature), `identities` (entry blob carries DPNS names, DashPay profile, contact profiles), `identity_keys` (public only), `contacts` (sent/received/established, alias/note/hidden, accepted accounts, payment_channel_broken), `ignored_senders`, `platform_addresses`, `asset_locks` (status + proof), `invitations`, `dpns_name_states`, `identity_scan_state`, `token_balances`, `dashpay_profiles` + `dashpay_payments_overlay`. Migrations V001…V018 (`P/rs-platform-wallet-storage/migrations/`). No secrets in SQLite (`P/rs-platform-wallet-storage/SECRETS.md`).

Attested capabilities (`persister.rs:1306-1333`): `ATOMIC_CHANGESETS | INVITATIONS | ASSET_LOCK_FUNDING_INDICES | UNSIGNED_TOKEN_STORAGE | PENDING_CONTACT_CRYPTO | DPNS_NAME_STATES | TRACKED_ASSET_LOCKS | TRACKED_MASTERNODES | CORE_SWEEP_REMOVAL | DASHPAY_PAYMENTS` (+ `SHIELDED_VIEWING_KEYS` with `shielded`). **Not `WALLET_RESTORE`.**

### 5.2 Gaps that matter for DashPay

| Gap | Evidence | Effect on desktop | Fix |
|---|---|---|---|
| `invitations` not rehydrated | `LOAD_UNIMPLEMENTED` `persister.rs:44-49` | Inviter's list is lost from platform-wallet memory after restart (rows stay in SQLite). | Engine reads `schema::invitations::read_all` (pub) for UI, or upstream loader. |
| No `WALLET_RESTORE` attestation | `persister.rs:1311-1313` | `create_invitation` refused (`INVITATION_CREATION`); `mark_asset_lock_consumption_unknown` refused (`ASSET_LOCK_RECONCILIATION`, `tracking.rs:330`; reached only on an unauthenticated "already consumed" report, `orchestration.rs:470-528`). | Upstream: wire loaders for invitations/token balances/overlay/pending crypto, then attest. Carry as `[patch]` on a same-repo platform branch (DESIGN R1 rule). Do **not** fake the bit in `WalletStore`. The bit protects against re-exporting a bearer voucher key after restart. |
| `pending_contact_crypto` has no reader | `persister.rs:37-41`; no trait method/`ClientStartState` field (`PW/changeset/client_wallet_start_state.rs`) | Queue lost on restart. It heals on the next DashPay sweep, which re-enqueues; until a signer-present drain runs, those contacts' payments wait. | Accept for now (iOS/Android FFI hosts have the same gap, `changeset.rs` doc on `pending_contact_crypto_added`). Drain right after unlock. |
| `token_balances`, DashPay overlay not reloaded | `persister.rs:33-37` | Overlay is display metadata; DashPay state rehydrates from the identity blob. | DESIGN-opus U5 interim: forced re-sync. |
| `dpns_name_states` session-scoped | `PW/changeset/traits.rs:495-510` | Marketplace departure classification on the first pass after restart. | Mirror in `dw-appdb` or upstream loader. |

---

## 6. rs-sdk: DAPI, proofs, context provider

- **DAPI client**: `dash_sdk::Sdk` over `rs-dapi-client` (gRPC/TLS via tonic + rustls). Mainnet/testnet seeds come from `dash-network-seeds` `evo_seeds`, filtered by recorded TLS probes (`P/rs-sdk/src/sdk.rs:125-160`). Ops note from Yappr (memory): a random node per request over about 30 addresses churns connections. Pinning 3–5 healthy nodes cut latency there. Consider it for DashPay sync, which issues many document queries.
- **Proofs**: every query is proof-verified (`rs-drive-proof-verifier`). DashPay/DPNS document proofs are deep: the library runs the DashPay loop on an 8 MB stack thread for this (`PW/manager/dashpay_sync.rs:76-85`). **Verified gap:** `dw-engine`'s runtime (`DWD/rust/crates/dw-engine/src/engine.rs:49-55`) does not set `thread_stack_size`, so workers have tokio's default 2 MiB stack. On-demand DashPay calls (send/accept contact request, profile fetch, `sync_now`) would run there, and iOS saw SIGBUS stack overflows in exactly these proof verifications at default stacks. The FFI uses 8 MiB workers (`PWF/runtime.rs` `WORKER_STACK_BYTES`). Fix: `builder.thread_stack_size(8 * 1024 * 1024)` (about a 5-minute change; covered by item 1 in §3).
- **Quorum keys**: trusted HTTP provider (iOS parity, DESIGN R2). SPV-derived provider is the M6 hardening item. dashmate local exposes the same JSON API (§8.2).
- **Protocol version**: auto-detect ratchet (`sdk.rs:387-483`). Seeds: PV13 mainnet/testnet/regtest, PV14 devnet. For a dashmate *local* (Core regtest) network running v5 drive (PV14), seed 14 explicitly (`SdkBuilder::with_initial_version`/`with_version`). The comment at `sdk.rs:60-64` says PV14 contracts use index grammar that PV13 cannot deserialise.
- **Proof envelope floor**: see §7.2, a security reason to bump.

---

## 7. Pin vs default branch

### 7.1 What changed (35 commits, `git log bc321362b9..origin/v5.0-dev`)

Wallet/SDK-relevant (diffstat over `rs-platform-wallet`, `-storage`, `-ffi`, `rs-sdk`: 25 files, +1738/−237; storage unchanged):

| Commit | What | Matters to desktop? |
|---|---|---|
| `451ed5cd4e` #4978 | DPNS: paged owned-name fetch (`get_all_dpns_usernames_by_identity`, +488 in `rs-sdk/src/platform/dpns_usernames/queries.rs`), `DpnsFetch`, `apply_fetched_dpns_names` (+271 `identity_ops.rs`), marketplace +496 | **Yes**: main-username retention and identities with many names; iOS ships it. |
| `3f029d8d30` #5206 | Withdrawal/transfer balance refusal → `InsufficientIdentityCredits` | **Yes**: iOS UI relies on it (`IdentityWithdrawViewModel.swift:181-193`). |
| `65e1969c6f` #5294 | Refuse GroveDB V0 proof envelopes at every protocol version (`MINIMUM_GROVEDB_PROOF_ENVELOPE_VERSION = 1`) | **Yes (security)**: at the pin PV13 accepts V0 (`system_limits/v1..v3.rs`, "V0 envelopes stay accepted until v14"), and both live networks are PV13. Head comment: "every live network serves V1" from PV12, so no compatibility risk. |
| `cf8cff9a68` #5305 | grovedb bump: limit-cut V1 proof hides its range bound | Yes (proof soundness); changes the grovedb rev in Cargo.lock. |
| `8929fc2ded`, `1bc76c0b36`, `7b61d6f3de` | test/CI | No |
| PV14 consensus features (#5227, #5228, #5237, #5239, #5250, #5284, #5295, #5014, #5316, #5318, #5319, #5321, #5325, #5326, `2643dc1cdb`, `8187374e70`) | Drive / dpp rules for protocol 14 | Only once a network activates PV14. Clients must track the network's version, so staying close to head is the safe default. |
| `c59af8f92f` | 5.0.0-beta.2 | — |

`Cargo.toml` at head also moves rust-dashcore `e4208c90 → 40268cc0` (5 commits, `rpc-json` only; desktop has no `dashcore-rpc` user) and grovedb `dce8252f → 9791d277`.

### 7.2 Recommendation

**Bump to v5.0-dev head (`bc41f1bc23`, or whatever head is when DashPay work starts) in one PR before any DashPay code.**

- Cost: edit 6 platform revs + 5 rust-dashcore revs in `DWD/rust/Cargo.toml` (they must stay identical to platform's pin, as noted at `Cargo.toml:28-29`), update `Cargo.lock` (platform, grovedb, rust-dashcore), full rebuild in the shared target dir (dev profile; the disk guard applies), then `cargo test`, the regtest L1/CoinJoin suites, and `swift test`. No `platform-wallet` API used by `dw-engine` changed: `error.rs` only gained variants and helpers, and `dw-engine/src/error.rs:205-238` matches specific variants with a fallback. Update DESIGN.md R3 pins. Estimate **0.5–1 day**.
- Do **not** target v5.1-dev now. It lacks the 18 newest v5.0 commits (including #4978, #5294, #5305) and introduces PV15. Its DashPay extras are #4623 (contact pay without Core funding, iOS PR waiting on v5.1), #4997 (invitation claim status, idempotent claim), #4764 (signing-key selection across identity ops, new `signing_key.rs`), and `f757b85888` (Swift-only perf). Re-evaluate when iOS moves to v5.1. Merging v5.0 into v5.1 is routine upstream (`c301cb60a5`, `824a75de3d`).
- Keep the rule of one coordinated bump per milestone. A moving pin would churn the 700-crate graph.

---

## 8. Network and test options

### 8.1 Testnet + faucet (primary)

- Platform testnet: PV13, drive 4.1.0-rc.2, about 9.5k identities. Explorer `https://testnet.platform-explorer.pshenmic.dev`. Quorum service `https://quorums.testnet.networks.dash.org`.
- Faucet `https://faucet.thepasta.org` (skill `dash-faucet`, `~/.claude/skills/dash-faucet/SKILL.md`). Live status 2026-10-08: `status ok`, `coreFaucetAmount 1`, `rateLimitPerHour 3` (soft 3 / turnstile 10 / hard 25), daily budget 200 tDASH, `invitationsEnabled true`, `invitationNetwork mainnet`, `invitationAmount 0.03`.
  - `POST /api/core-faucet {address}` gives 1 tDASH to a `y…` address (CAP token may be required; solve in the web UI, never automate around it).
  - `POST /api/asset-lock-proof {assetLockPublicKey}` gives `assetLockProof`, `txid`, `creditsAmount` for a locally generated key. The private key stays local. This funds an identity with **no L1 balance**. Desktop wiring: it is the same shape as an invitation voucher (external key + proof). Reuse `claim_invitation`'s raw-key path or `dash_sdk` `PutIdentity` with the proof. Developer/test-only, behind a dev toggle (iOS has a "Get Test Dash" shortcut; IOS-095).
  - Mainnet invitations (0.03 DASH, 1 h expiry): real-money end-to-end test of IOS-077 claim. Use sparingly.
- Test plan (DESIGN-opus §WS-07 nightly suite): two `dwcli` identities on testnet → register + DPNS → contact request round trip → accept → pay contact both ways → profile update → contactInfo alias → invitation claim. Two wallets need about 2–3 tDASH each (identity about 0.003 DASH in credits min; a non-contested name is cheap; a contested name costs about 0.2 DASH plus a voting period of about 2 weeks on mainnet, shorter on testnet — UNVERIFIED testnet duration).

### 8.2 Local devnet via dashmate (secondary, heavy)

- `P/dashmate` (5.0.0-beta.1 at the pin). `dashmate setup local` creates a seed node plus N masternodes (default 3) on Core **regtest** (`NETWORK_LOCAL`), each running core + drive-abci + tenderdash + rs-dapi + envoy gateway (`P/dashmate/docker-compose.yml`). The seed also runs `quorum_list` (`docker-compose.yml:215-243`, port 22444, enabled by `src/listr/tasks/setup/setupLocalPresetTaskFactory.js:177`), the same API as `quorums.*.networks.dash.org`. The local preset forces BIP157 compact filters on for SPV clients (`configs/defaults/getLocalConfigFactory.js:35-46`).
- Desktop wiring needed: `DashNetwork::Regtest` + explicit `dapi_addresses` (gateway, self-signed TLS → add a CA-cert option to `build_sdk`), `quorum_url = http://127.0.0.1:22444` (http allowed off mainnet/testnet), SPV peers = local cores, `with_initial_version(14)`.
- Cost: Docker images about 5–8 GB; about 4 × (dashd + drive + tenderdash + dapi + envoy), roughly 8–12 GB RAM and 20–40 GB disk (UNVERIFIED estimate); setup 30–60 min (mines, registers masternodes, waits for quorums). The laptop disk is tight (`project_laptop_worktree_prune` memory), so run it on agentbox (VM 250) or the Studio, never the developer's Mac. Image tags must match the v5 drive/dapi versions (`dashpay/drive:5.0.0-beta.x`, UNVERIFIED availability on Docker Hub; otherwise build with `docker-compose.build.*.yml`, which takes hours).
- Value: deterministic contests (short voting periods), invitations without mainnet money, PV14 rehearsal. Use it for CI-like repeatability only after testnet flows work.

### 8.3 Regtest (`DWD/regtest`, no Platform)

- Existing harness: dashd functional framework with masternodes, quorums, ChainLocks and InstantSend (`DWD/regtest/README.md:10,93-100,201-227`).
- Useful for the Core half only: asset-lock special transaction (type 8) building and broadcast, IS-lock wait, IS→CL fallback timing (`registration.rs` 300 s / 180 s bounds; make them configurable for tests), funding-source pooling, `DrainAccountBalance`. Platform submission is not possible.

---

## 9. Scope notes (product)

- In scope (mobile parity): rows 1–14, 16, 17, and the shielded pool (19) later. Invitation creation (15) needs a decision: Android has it, iOS removed it, DESIGN D11 says IN, and the library blocks it on SqlitePersister.
- Out of scope (Dash Core keeps it, per `DWD/CLAUDE.md`): masternode contested-name voting (IOS-079), evonode tools/withdrawals/ProUpServTx (IOS-080…083), masternode keychain. Show only the user's own contest state (IOS-068).
- DashPay Connect / `dashid:` browser login (iOS scheme `dashid`, SwiftExampleApp BLE prototype) is not a shipped iOS feature; leave it out (memory: DashPay Connect v2 is still a plan).

---

## 10. Third-party APIs used by the mobile apps

No secret values are reproduced here. "Key source" says where each app gets credentials. On this laptop the iOS plists `Coinbase-Info`, `SwapKit-Info`, `Topper-Info`, `Uphold-Info` are present but empty placeholders; `ZenLedger-Info.plist` has `CLIENT_ID`/`CLIENT_SECRET` keys.

| Service | Endpoints (iOS file) | Auth | Key source iOS / Android | Desktop notes |
|---|---|---|---|---|
| **Uphold** | `https://api.uphold.com/`, authorize `https://uphold.com/authorize/<client_id>?scope=…`, tx `https://uphold.com/reserve/transactions/%@`; sandbox `api-sandbox.uphold.com` (`IOS/Models/Uphold/DWUpholdMainnetConstants.m`, `DWUpholdConstants.m`) | OAuth2 auth-code via `ASWebAuthenticationSession` with callback scheme `dashwallet` (`IOS/UI/Swap/EnterAddress/EnterAddressHostingController.swift:166-180`, `IOS/UI/Buy Sell/IntegrationViewController.swift:278-308`); client secret used at token exchange; OTP on transfers | iOS: mainnet from git-ignored `Uphold-Info.plist`; **sandbox client id/secret are committed** in `DWUpholdConstants.m`. Android: `UPHOLD_CLIENT_ID/SECRET` buildConfig from `service.properties` (`AND/wallet/build.gradle:220-227,360-361`), testnet defaults committed in `build.gradle:263-264`. | Redirect URI is registered on Uphold's side (UNVERIFIED value; iOS matches on "uphold" in the callback). Desktop needs a custom scheme or loopback redirect allowed by Uphold's app config. |
| **Coinbase** | `https://api.coinbase.com` (v2), `https://login.coinbase.com` (OAuth), token `https://api.coinbase.com/oauth/token` (`IOS/Models/Coinbase/Infrastructure/API/*`) | OAuth2 code, redirect `dashwallet://brokers/coinbase/connect`, scopes `wallet:accounts:read,…,wallet:transactions:send,…` (`Coinbase+Constants.swift:22-35`); 2FA on send | iOS `Coinbase-Info.plist` (`CLIENT_ID`, `CLIENT_SECRET`); Android `COINBASE_CLIENT_ID/SECRET` from `service.properties` (`build.gradle:348-349`); same redirect (`AND/integrations/coinbase/.../CoinbaseConstants.kt:41`) | Same redirect works if the desktop registers `dashwallet:` (macOS `CFBundleURLTypes`; Windows `HKCU\Software\Classes\dashwallet` written by the MSI; Linux `.desktop` `MimeType=x-scheme-handler/dashwallet`, Flatpak OK). A loopback redirect (`http://127.0.0.1:<port>/…`) must be added to the Coinbase OAuth client by DCG. Client secret in a desktop binary is as exposed as on mobile; prefer PKCE if Coinbase allows it. |
| **Topper** | widget `https://app.topperpay.com/` (sandbox `app.sandbox.topperpay.com`), `https://api.topperpay.com/assets/crypto-onramp`, `/payment-methods/crypto-onramp` (`IOS/Models/Uphold/Topper.swift:31-35`) | No OAuth: the app signs an **ES256 JWT** (kid = key id, sub = widget id) with a private key it ships (`Topper.swift:103-126`) and opens the widget URL | iOS `Topper-Info.plist` (`KEY_ID`, `WIDGET_ID`, `PRIVATE_KEY`, `SANDBOX_*`; `IOS/UI/Uphold/TopperViewModel.swift:35-41`); Android `TOPPER_KEY_ID/WIDGET_ID/PRIVATE_KEY` buildConfig (`build.gradle:366-368`) | Opens in the system browser; no redirect needed. Shipping the signing key in a desktop binary is extractable (same as mobile). Ask DCG/Topper about a desktop widget id or server-side signing. |
| **Maya / SwapKit** (swap DASH ↔ other chains) | Maya: `https://mayanode.mayachain.info/mayachain/`, `https://midgard.mayachain.info/v2/`, explorer `mayascan.org`, NEAR `explorer.near-intents.org` (`IOS/Models/Maya/*`); SwapKit `https://api.swapkit.dev/` (`IOS/Models/SwapKit/SwapKitEndpoint.swift`) | Maya: none. SwapKit: header `x-api-key` | SwapKit: iOS `SwapKit-Info.plist` `API_KEY`; Android `SWAPKIT_API_KEY` from `service.properties` (`AND/integrations/maya/build.gradle:12-30`) | Pure HTTP. Deposit tx with OP_RETURN memo exists in the engine plan (iOS `SwiftDashSDKTransactionSender.swift:147`). |
| **CrowdNode** | `https://app.crowdnode.io/` (test `test.crowdnode.io`), login `login.crowdnode.io` (`logintest…`), `odata/apifundings/*`, `odata/apiaddresses/*`, `odata/apimessages/SendMessage(…signature…)` (`IOS/Models/CrowdNode/API/CrowdNodeEndpoint.swift:49-57`, `CrowdNode+Constants.swift`) | No API key: signup/deposit/withdraw are on-chain txs to CrowdNode's address plus a **signed message** (`signmessage`, already in `dw-vault`) | none | Online-account linking opens a web login (no redirect back; iOS polls the address status). DESIGN puts CrowdNode behind a flag. |
| **CTX (DashSpend gift cards)** | `https://spend.ctx.com/` (staging `staging.spend.ctx.com`), paths `login`, `verify-email`, `refresh-token`, `gift-cards`, `merchants/{id}` (`IOS/Models/Explore Dash/Services/DashSpend/CTX/CTXSpendEndpoint.swift:44-110`) | Email OTP → bearer + refresh token; header `X-Client-Id: dcg_ios` (Android `dcg_android`, `AND/features/exploredash/.../CTXSpendConstants.kt:22-23`); purchase paid by BIP70 | Client id is a constant, not a secret | Ask CTX for `dcg_desktop` (or reuse). BIP70 is app code (iOS `BIP70PaymentService+App.swift`). |
| **PiggyCards** | `https://api.piggy.cards/dash/v1/` (dev `apidev.piggy.cards`), `signup`, `login`, `verify-otp`, `brands/{country}`, `giftcards/{country}`, `orders`, `exchange-rate` (`PiggyCards/PiggyCardsEndpoint.swift`) | Email OTP → bearer (expires 3600 s) | none | Geo restriction via `https://ip-api.com/json/` (`GeoRestrictionService.swift`). |
| **Explore merchant/ATM DB** | Firebase Storage `gs://dash-wallet-firebase.appspot.com/explore/explore-v4.db` / `explore-v4-testnet.db` (iOS, `ExploreDatabaseSyncManager.swift:52-56`); Android `explore/explore.db` / `explore-testnet.db` (`AND/features/exploredash/.../ExploreRepository.kt:59-68`) | iOS: Firebase SDK (`[FIRApp configure]`, `IOS/../AppDelegate.m:139`), no explicit sign-in. Android: `signInAnonymously()` (`ExploreRepository.kt:124`). Zip encrypted with the password stored in object metadata `Data-Checksum` (`ExploreDatabaseSyncManager.swift:25,176,217-239`) | iOS `GoogleService-Info.plist` (git-ignored); Android `google-services.json` | Desktop: Firebase Storage REST (`https://firebasestorage.googleapis.com/v0/b/dash-wallet-firebase.appspot.com/o/explore%2Fexplore-v4.db?alt=media`; metadata without `alt`). Works only if storage rules allow unauthenticated reads; otherwise use Identity Toolkit anonymous sign-up with the web API key (UNVERIFIED which). Needs a zip reader with ZipCrypto/AES password support. |
| **Imgur** (avatar upload) | `https://api.imgur.com/3/upload`, `/3/image/{deleteHash}` (`IOS/Infrastructure/Networking/DWAvatarUploadClient.swift`) | `Authorization: Client-ID <id>` (anonymous upload) | iOS `Imgur-Info.plist` `IMGUR_CLIENT_ID`; Android `IMGUR_CLIENT_ID/SECRET` (`build.gradle:290-291`) | Also Gravatar URL option (no key). |
| **ZenLedger** (tax export, IOS-040) | `https://api.zenledger.io` (`IOS/Models/Taxes/ZenLedger.swift`) | client-credentials token, then bearer | iOS `ZenLedger-Info.plist`; Android `ZENLEDGER_CLIENT_ID/SECRET` | Client secret in binary. |
| Platform/Core infra | DAPI evo seeds, `quorums.*.networks.dash.org`, Insight (`InsightExplorerAPI.swift`, phrase repair only) | none | — | Already in engine (§2.1). |

OAuth on desktop in general: both apps use the custom scheme `dashwallet://`. Android also registers `dashpay://invite`, `dash:`, `pay:`, `dash-key:`, `dash-st:` (`AND/wallet/AndroidManifest.xml:134-250`); iOS registers `pay`, `dash`, `dashwallet`, `dashid` plus universal links on `invitations.dashpay.io`. Desktop options: (a) register `dashwallet:` and `dashpay:` per OS (cheapest; same provider configs as mobile); (b) RFC 8252 loopback redirect (needs new redirect URIs registered by DCG with Coinbase/Uphold). Use the system browser for the auth page, never an embedded webview (both providers discourage webviews).

---

## 11. Work plan implied (engine side)

1. Pin bump to v5.0-dev head (0.5–1 d).
2. `platform/signers.rs` + `SignerScope::Platform` + `platform-encryption` dep (2–3 d).
3. Startup ordering + sync loops + Platform events + Sync Info (2–3 d).
4. Identity register/restore/top-up/withdraw with the iOS key policy, resume and recovery (6–9 d).
5. DPNS register/search/availability + own contest status (4–6 d).
6. Contacts read model, send/accept/ignore, contactInfo, profile, avatar (8–11 d).
7. Contact payments + history attribution (2–3 d).
8. Invitation claim (2–3 d); create only after the storage patch (upstream 3–5 d).
9. Platform addresses (3–5 d), marketplace (4–6 d), shielded (10–15 d) as later slices.
10. Testnet nightly suite with two `dwcli` identities + faucet (3–4 d).

---

## 12. Open questions

1. **Invitation creation**: build it (Android parity, DESIGN D11) with an upstream `platform-wallet-storage` patch (loaders + `WALLET_RESTORE`), or drop it like iOS?
2. **Pin timing**: bump to v5.0-dev head now (before M4) or at M4 kickoff? When iOS adopts v5.1-dev (#4623 any-balance contact pay, #4997 claim status), does desktop follow straight away?
3. **Shielded**: when to turn on the `shielded` feature (build-time and binary-size cost) given iOS shows a Shielded balance by default?
4. **Credentials for desktop**: will DCG register desktop redirect URIs (Coinbase, Uphold), issue a CTX client id, and decide whether Topper/ZenLedger/Imgur keys may ship in desktop binaries (or move behind a server)?
5. **Explore DB access**: do the Firebase Storage rules allow unauthenticated reads, or does desktop need anonymous auth with the web API key?
6. **Local devnet host**: agentbox or Studio for dashmate, and are v5 drive/dapi images published for the pin?
7. **Trust model**: stay on the trusted quorum service for 1.0 (iOS parity) and move to the SPV-derived quorum provider in M6, as DESIGN R2 says?
