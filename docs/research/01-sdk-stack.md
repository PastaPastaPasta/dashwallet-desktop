# 01 — SDK stack inventory (SwiftDashSDK + Rust FFI) for a desktop Dash wallet

Status: research, read-only. Written 2026-10-05.

## Key findings (TL;DR)

1. **One Rust archive, four C headers.** SwiftDashSDK links only `librs_unified_sdk_ffi.a` (crate `rs-unified-sdk-ffi`, = `rs-sdk-ffi` + `platform-wallet-ffi` + `key-wallet-ffi` + `dash-network`). SPV is reached through `platform-wallet-ffi`, which embeds `dash-spv`. `dash-spv-ffi` is not shipped. About 894 exported C functions; cbindgen headers are plain C, so any language can consume them.
2. **macOS arm64 is already a supported slice** (`build_ios.sh --target mac`; `Package.swift` declares macOS 15; `swift test` runs on the Mac). There is **no Intel slice**: a `BUILD_INTEL_MAC` flag exists but nothing sets it. Adding one needs an x86_64 cargo build and a `lipo`. Linux and Windows builds of the same crate (`staticlib` + `cdylib`) look feasible: there are no `cfg(target_os)` blockers and no rocksdb, and Android builds of the same engine prove it works off Apple. The open issues are OpenSSL on Linux (reqwest native-tls), libclang plus protoc at build time, and an MSVC compile nobody has tried. Linux compiles in platform CI (clippy/tests); Windows is untested.
3. **The Swift package is Apple-only** (SwiftData in 154 files, including the central `PlatformWalletManager` and the 11.6k-line persistence handler; Security Keychain; Combine; CryptoKit). It is fine for a macOS SwiftUI app. It is not reusable on Linux or Windows without rewriting persistence, secrets and observation.
4. **Feature coverage is strong for Platform** (identities, DPNS + marketplace + contest voting, DashPay, tokens, credits, Platform addresses, Orchard shielded pool). L1 has a mature SPV client (headers, BIP157/158, MN lists, BLS-verified ChainLocks/ISLocks) and good tx building (coin control, selection strategies, reservations, P2P broadcast with acceptance detection).
5. **Gaps that matter for replacing dash-qt.** Missing: a CoinJoin mixing client (only the account type and detection exist), Core governance (proposals and votes), ProRegTx/ProUpRegTx/ProUpRevTx builders (only the ProUpServTx revive exists), multisig/PSBT wallet flows, hardware wallets, BIP21 parsing (BIP70 lives in app code), a payee address book, message verification over FFI, dynamic fee estimation, and wallet-at-rest encryption/backup files. The iOS app does without all of these.
6. **Persistence belongs to the host.** Rust emits changesets through a 46-slot C callback vtable (37 base + 9 extension), and the iOS app implements it with SwiftData. `rs-platform-wallet-storage` already provides a Rust SQLite persister plus OS-keyring/Argon2 vault with Linux and Windows backends, but **nothing links it yet**. It is the obvious cross-platform path.
7. **Trust model to note:** the Swift SDK always uses the *trusted* HTTPS quorum service (`quorums.<net>.networks.dash.org`) for Platform proof verification, not SPV quorum data.

## 0. Revisions examined

| Repo | Revision | Notes |
|---|---|---|
| dashpay/platform | `origin/v5.0-dev` @ `bc321362b9` (2026-10-04), clean worktree `/Users/pasta/workspace/dashwallet-desktop-deps/platform` | **Primary target.** All `packages/...` paths below are relative to this worktree unless noted. |
| dashpay/platform | `v4.3-dev` @ `67340ad824` (2026-09-28) | The main checkout `/Users/pasta/workspace/platform` (dirty: 787 staged files, unmerged paths, belongs to someone else — not touched). It is what dashwallet-ios local builds use today (§6). Diffed against v5.0-dev in §0.1. |
| dashpay/rust-dashcore | `e4208c90786a6854bd498315bcb571ef24182c15` (2026-09-10, "fix(dash-spv): stop losing derived scripts…") | Git dependency pinned **identically** on both platform branches (`Cargo.toml:68-75`). The local checkout `/Users/pasta/workspace/rust-dashcore` (`dev` @ `1a6fb3bf`) is **19 commits older** than the pin, so it was not used; the pin was extracted with `git archive` instead. |
| dashpay/dashwallet-ios | `develop` @ `7c0064d6b2` (2026-10-02), clean | §6. |

Rust toolchain pinned to `1.98.1` (`rust-toolchain.toml`).

### 0.1 v4.3-dev vs v5.0-dev differences that matter here

- `packages/swift-sdk/Package.swift` and `packages/rs-unified-sdk-ffi/Cargo.toml` are byte-identical.
- `build_ios.sh` differs by one line: v5.0 honours `CARGO_TARGET_DIR` (`build_ios.sh:24-26`), v4.3 hard-codes `$ROOT_DIR/target`.
- Exported FFI symbol set is essentially the same: rs-sdk-ffi 216→217 `#[no_mangle]`, platform-wallet-ffi 416→415. Added in v5.0: `dash_sdk_data_contract_check_property_constraints`, `dash_sdk_data_contract_get_property_constraints`. Removed: `dash_sdk_document_destroy`, `platform_wallet_invitation_claim_status`.
- Notable v5.0-only commits touching the stack (`git log 67340ad824..bc321362b9`): token shielded pools (`691d71e331`, #4760, breaking), PV14 propertyConstraints rules in Swift SDK (#5064/#5098/#5121), SPV stop off the main thread + serialized SPV stops (`bb4e7de380`, `989b42b1d3`, `39631f4ec3`), App Store SwiftData schema 3.0.0 freeze (`a31bce178d`), consensus error codes reaching Swift/Kotlin (#5116). v4.3-dev and v5.0-dev have diverged (v4.3 is not an ancestor of v5.0; dashwallet-ios local checkout is 196 behind / 33 ahead of v5.0-dev).
- Swift SDK grew from 218 files / 73.8k lines (v4.3) to 257 files / 78.9k lines (v5.0); almost all growth is in `Persistence/` (frozen schema snapshots).

---

## 1. Layering

### 1.1 What SwiftDashSDK links

```
SwiftDashSDK (Swift, packages/swift-sdk/Sources/SwiftDashSDK, 257 files, ~78.9k lines)
   └── binaryTarget DashSDKFFI  (DashSDKFFI.xcframework, Package.swift:18-21)
          └── librs_unified_sdk_ffi.a   (crate rs-unified-sdk-ffi, staticlib+cdylib)
                 ├── rs-sdk-ffi            (platform)  dash_sdk_*  — DAPI/Platform SDK, proofs, signer & mnemonic-resolver vtables
                 ├── platform-wallet-ffi   (platform)  platform_wallet_*, core_wallet_*, asset_lock_manager_*, … — the wallet engine incl. SPV
                 │      └── platform-wallet (rs-platform-wallet) ── dash-spv, key-wallet, key-wallet-manager, dash-sdk, dpp
                 ├── key-wallet-ffi        (rust-dashcore) wallet_*, mnemonic_*, derivation_*, address_*, transaction_* …
                 └── dash-network (feature "ffi") (rust-dashcore) FFINetwork enum
```

- `packages/rs-unified-sdk-ffi/Cargo.toml:7-14` — `crate-type = ["staticlib", "cdylib"]`; deps are exactly `key-wallet-ffi`, `platform-wallet-ffi`, `rs-sdk-ffi`, `dash-network[ffi]`. `src/lib.rs:1-4` is four `pub use` lines; the crate only exists to merge the four FFI crates into one archive so shared deps (secp256k1, tokio, …) appear once.
- Features: `shielded` → `platform-wallet-ffi/shielded` (Orchard; off by default at crate level, **on** in the iOS build, `build_ios.sh:237-242`); `tokio-metrics` (dev builds only).
- **`dash-spv-ffi` is NOT linked.** It is absent from `rs-unified-sdk-ffi/Cargo.toml` and from `build_ios.sh` `INCLUDED_CRATES` (`build_ios.sh:33-38`: `dash-network`, `key-wallet-ffi`, `rs-sdk-ffi`, `platform-wallet-ffi`). No Swift or platform-wallet-ffi code calls `dash_spv_*` (the only hit is a stale doc comment, `Sources/SwiftDashSDK/DashNetwork.swift:9`). SPV is driven through **platform-wallet-ffi** (`src/spv.rs`), which embeds `dash-spv` as a Rust library via `rs-platform-wallet/src/spv/{mod,runtime,peers}.rs`.
- `platform-wallet-ffi` itself depends on `rs-sdk-ffi` (for `SignerHandle`/`VTableSigner` and `MnemonicResolverHandle`; `rs-platform-wallet-ffi/Cargo.toml:20-23`).
- Platform SDK context provider: Swift always creates the SDK with **`dash_sdk_create_trusted`** (`Sources/SwiftDashSDK/SDK.swift:258-349`), i.e. quorum public keys come from an HTTPS quorum service — defaults `https://quorums.{mainnet,testnet}.networks.dash.org` (`packages/rs-sdk-trusted-context-provider/src/lib.rs:7-9,26-27`), custom URL must be https (`provider.rs:191`). The SPV masternode list is **not** used as the Platform proof context provider. This is a trust decision the desktop wallet inherits unless it wires `dash_sdk_create_with_callbacks`/`dash_sdk_context_provider_from_callbacks` (`rs-sdk-ffi/src/sdk.rs:578`, `context_provider.rs:54`) to SPV quorum data itself.

FFI surface size at v5.0-dev (`#[no_mangle]` count):

| Crate | Source | Exported fns | C header (cbindgen) |
|---|---|---|---|
| rs-sdk-ffi | `packages/rs-sdk-ffi/src` (163 files, 44.3k lines) | 217 | `rs-sdk-ffi/rs-sdk-ffi.h` (4,345 lines in the Aug-12 macOS slice) |
| platform-wallet-ffi | `packages/rs-platform-wallet-ffi/src` (110 files, 59.0k lines) | 415 | `platform-wallet-ffi/platform-wallet-ffi.h` (7,898 lines) |
| key-wallet-ffi | rust-dashcore `key-wallet-ffi/src` | 261 | `key-wallet-ffi/key-wallet-ffi.h` (5,225 lines) |
| dash-network | rust-dashcore `dash-network` | 1 | `dash-network/dash-network.h` (33 lines) |
| dash-spv-ffi (not shipped) | rust-dashcore `dash-spv-ffi/src` | 44 | — |

Biggest platform-wallet-ffi groups (exported fns per file): `manager_diagnostics.rs` 28, `shielded_send.rs` 19, `core_wallet/transaction_builder.rs` 19, `dashpay.rs` 16, `wallet.rs` 15, `dpns_marketplace.rs` 15, `shielded_sync.rs` 14, `manager.rs` 14, `identity_sync.rs` 13, `managed_identity.rs` 12, `dpns.rs` 11, `contact_request.rs` 11, `spv.rs` 10, `platform_addresses/wallet.rs` 10, `established_contact.rs` 10. `persistence.rs` exports few functions but defines the host-callback vtable: `PersistenceCallbacks` (37 `*_fn` slots, `persistence.rs:600-1160`) + `PersistenceCallbacksExtension` (9 slots, `:286-400`); 119 `extern "C" fn` mentions in the file overall — see §4.

### 1.2 How the xcframework is built (`packages/swift-sdk/build_ios.sh`, 342 lines)

1. Flags `--target ios|sim|mac|all|tests`, `--profile dev|release` (`:106-138`). The profile is rewritten to `dev-ios`/`release-ios` (`:156`). `release-ios` = `inherits release, panic=abort, strip=symbols, lto=fat, codegen-units=1, opt-level=3` (root `Cargo.toml:81-87`); `dev-ios` = `dev` + `panic=abort` + line tables (`:91-94`) — the script warns dev builds abort on any `debug_assert!` and must not ship (`build_ios.sh:162-172`).
2. Per target: `cargo build -p rs-unified-sdk-ffi --profile $PROFILE --target <triple> --features "shielded[ tokio-metrics]"` (`:249-292`). Triples: `aarch64-apple-ios`, `aarch64-apple-ios-sim`, **`aarch64-apple-darwin`** (`:279-292`).
3. Headers: each FFI crate's `build.rs` runs cbindgen and writes `target/<triple>/<profile>/include/<crate>/<crate>.h` (e.g. `rs-platform-wallet-ffi/build.rs`, `.ancestors().nth(3)` → `target/<triple>/<profile>`). `inject_modulemap` (`:191-235`) prunes header dirs not in `INCLUDED_CRATES`, checks all four exist, writes an umbrella `DashSDKFFI.h` that `#include`s them in dependency order, and a `module.modulemap` (`module DashSDKFFI { umbrella header "DashSDKFFI.h"; export * }`).
4. `xcodebuild -create-xcframework -library … -headers …` per slice → `DashSDKFFI.xcframework` (`:297-304`).
5. Then it builds `SwiftExampleApp` for the iOS simulator with `-warnings-as-errors` as a smoke check (`:311-342`; `SKIP_EXAMPLE_APP_BUILD=1` skips).
- `run_tests.sh:146-148` runs `build_ios.sh --target tests --profile dev` (= sim + mac) and then **`swift test` on the host Mac**, so the SwiftPM package is exercised on macOS in CI/local test runs.
- Release CI: `.github/workflows/release-swift-sdk.yml:220` runs `build_ios.sh --target all --profile release` on a self-hosted macOS ARM64 runner, zips the xcframework and attaches it to the platform GitHub release with an SPM checksum (`:222-245`). PR CI (`swift-sdk-build.yml:56`) only installs iOS targets.
- Build-host requirements: Xcode, `protoc` (dapi-grpc's `build.rs` compiles protos with `tonic-prost-build`, `packages/dapi-grpc/build.rs:522,573`; CI installs protoc 32.0), libclang (bindgen build-dep of `rs-x11-hash 0.1.8`, see §1.4).

### 1.3 macOS slice — exists today (arm64 only)

- `Package.swift:6-10` already declares `.iOS(.v18), .macOS(.v15)`; `build_ios.sh --target mac` produces a `macos-arm64` slice; `MACOSX_DEPLOYMENT_TARGET` defaults to 15.0 (`:6`).
- Evidence it works: the main checkout's (gitignored) `DashSDKFFI.xcframework` contains `macos-arm64/librs_unified_sdk_ffi.a` — 731 MB, `lipo -info` → arm64, built 2026-08-12, plus `ios-arm64-simulator`. (Static archive size before app-link dead-stripping.)
- **No x86_64 (Intel) slice.** `BUILD_INTEL_MAC` is declared (`build_ios.sh:46`) and tested in validation (`:149`) but never set by any flag — dead code. To add Intel/universal macOS:
  1. `rustup target add x86_64-apple-darwin`;
  2. `cargo build -p rs-unified-sdk-ffi --profile release-ios --target x86_64-apple-darwin --features shielded`;
  3. `lipo -create` the arm64 and x86_64 `.a` into one universal archive (headers are identical — cbindgen output is target-independent; UNVERIFIED for any `#[cfg(target_arch)]`-gated items, none observed);
  4. pass that single universal `.a` as the macOS `-library` (xcframework allows one library per platform variant, so it must be lipo'd, not two `-library` entries) → slice `macos-arm64_x86_64`.
  No Intel-specific code blockers were found; native C deps (secp256k1-sys, blst, ring, bundled SQLite, x11 C) all support x86_64 macOS.
- Linking a macOS app against the archive needs the system frameworks Rust deps pull in: `SystemConfiguration` (already declared, `Package.swift:28`, used by reqwest/hyper proxy detection via `system-configuration-sys`), `Security` and `CoreFoundation` (via `security-framework`/`native-tls`), libc++/libSystem (auto). UNVERIFIED that this list is exhaustive.

**Local build attempt (2026-10-05):** `cargo build -p rs-unified-sdk-ffi --release --features shielded --target aarch64-apple-darwin` was started in the clean worktree with a free-disk watchdog. It compiled ~400 crates with no errors in 140 s, then the watchdog killed it because the laptop volume dropped below 3.5 GB free (started at 8 GB; other workloads were also consuming disk). The partial `target/` (952 MB) was deleted. **Result: inconclusive for v5.0-dev** — no compile error was seen, but the build did not finish. `swift build` of the package was not attempted (needs the xcframework). Re-run on a machine with ≥ 30 GB free (UNVERIFIED estimate for a release + LTO single-target build). Logs: `/Users/pasta/workspace/dashwallet-desktop-deps/build-logs/`.

### 1.4 Linux and Windows libraries of the same FFI

`rs-unified-sdk-ffi` already declares `staticlib` + `cdylib`, so `cargo build -p rs-unified-sdk-ffi --release --features shielded --target <triple>` yields `librs_unified_sdk_ffi.{a,so}` on Linux and `rs_unified_sdk_ffi.{lib,dll}` on Windows. Headers come out of the same cbindgen `build.rs` paths. What is known about portability:

- **No rocksdb** in the unified dependency graph (`cargo tree -p rs-unified-sdk-ffi --features shielded`, 692 unique packages; rocksdb is only in the Drive/server side).
- Native/C deps in the graph: `secp256k1-sys 0.10.1` (cc), `blst 0.3.12` (C + asm, via blsful/blstrs_plus for BLS), `ring 0.17.14`, `libsqlite3-sys 0.36.0` with `bundled` (via `rusqlite` from `grovedb-commitment-tree` for the shielded store), `rs-x11-hash 0.1.8` (C via cc **and `bindgen 0.65` → `clang-sys`**, so libclang must be installed on the build host), `image 0.25` (pure Rust png/jpeg/gif; used for DIP-15 avatar hashing).
- **TLS differs per OS:** `reqwest 0.12` (used by `rs-sdk-trusted-context-provider` → `rs-sdk-ffi`) uses `default-tls` = `native-tls`: Security.framework on macOS, **`openssl-sys` on Linux** (`cargo tree --target x86_64-unknown-linux-gnu -i openssl-sys`), **`schannel` on Windows**. So Linux builds need OpenSSL dev headers (or a vendored-openssl/rustls feature switch), and the shipped `.so` would dynamically link libssl. tonic/DAPI uses rustls (`rustls 0.23`, `rustls-native-certs`).
- Proof the stack builds off Apple: `rs-unified-sdk-jni` (`packages/rs-unified-sdk-jni/Cargo.toml`, cdylib `dash_sdk_jni`) wraps the same `rs-sdk-ffi` + `platform-wallet-ffi` + `key-wallet-ffi` and is built for `aarch64-linux-android` / `x86_64-linux-android` by `packages/kotlin-sdk/build_android.sh` (cargo-ndk) with `default = ["shielded"]`. Android is Linux/bionic. In addition, platform CI compiles the wallet FFI on **Linux x86_64 hosts**: `.github/workflows/tests-rs-wallet.yml:218-244` runs `cargo clippy` over `platform-wallet`, `platform-wallet-storage`, `platform-wallet-ffi`, `rs-unified-sdk-ffi`, `rs-unified-sdk-jni` and `cargo nextest` over the wallet crates with `--all-features` on a self-hosted Linux image (the job is confusingly named `test-mac`). So a Linux `cargo build` of the unified crate is expected to work; producing and linking the `.a`/`.so` into a desktop app is not exercised. **Windows is not built by any platform CI job** (UNVERIFIED that it compiles). rust-dashcore's own CI does test on ubuntu, ubuntu-arm, macOS **and windows-latest** (`.github/workflows/rust.yml:61-81` in the pinned tree), which covers key-wallet/dash-spv/key-wallet-ffi but not grovedb/orchard/dash-sdk.
- rust-dashcore crates (dash, dash-spv, key-wallet, key-wallet-manager, FFI crates) contain **no `cfg(target_os)`** code at all; only `target_arch` SIMD paths in `hashes/` with scalar fallbacks. dash-spv storage is plain files with a `File::try_lock` lock file (`dash-spv/src/storage/lockfile.rs:25`).
- Windows-specific unknowns (all UNVERIFIED): MSVC compile of `rs-x11-hash`'s `x11_hash.c`; bindgen/libclang on Windows; grovedb-commitment-tree / halo2 on MSVC; path handling in SPV/shielded stores. Platform build scripts also assume bash.
- Swift is not needed for a Linux/Windows build of the FFI. A cdylib normally exports the `#[no_mangle]` symbols of its dependency crates; that the unified `.so`/`.dll` exports all ~894 symbols was not verified (check with `nm -D` / `dumpbin /exports`).

---

## 2. Swift portability

### 2.1 Imports in `packages/swift-sdk/Sources` (v5.0-dev; files containing `import X`)

| Import | Files | Where | Linux/Windows (swift-corelibs) |
|---|---|---|---|
| Foundation | 256 | everywhere | available (swift-corelibs-foundation) |
| **SwiftData** | **154** | 148 under `Persistence/` (36 `@Model` types + 109 frozen schema snapshot files), plus `PlatformWallet/PlatformWalletManager.swift`, `PlatformWallet/PlatformWalletPersistenceHandler.swift`, `FFI/KeychainSigner.swift`, `FFI/TokenBalanceRefresh.swift`, `Core/Utils/DataContractParser.swift`, `Services/DataManager.swift` | **Apple-only** |
| DashSDKFFI (C module) | 72 | wrappers | portable if a modulemap + lib is supplied |
| Security (Keychain) | 4 | `Core/Wallet/WalletStorage.swift`, `Security/KeychainManager.swift`, `Security/KeychainInspector.swift`, `FFI/MnemonicResolverAndPersister.swift` | **Apple-only** |
| CryptoKit | 4 | `Core/Services/SDKLogger.swift`, `Utils/TestnetFaucet.swift`, `Persistence/DashLegacyStoreSQLite.swift`, `Helpers/TestKeyGenerator.swift` | Apple-only (swift-crypto is the portable substitute) |
| os.log | 3 | `SDK.swift`, `Security/KeychainManager.swift`, `PlatformWallet/PlatformWalletManager.swift` | Apple-only |
| CoreData | 2 | `Persistence/DashLegacySchemaBridge.swift`, `Persistence/DashModelContainer.swift` | Apple-only |
| SQLite3 | 1 | `Persistence/DashLegacyStoreSQLite.swift` | system sqlite3 module; needs a modulemap on Linux |
| LocalAuthentication | 1 | `Core/Wallet/WalletStorage.swift` (`LAContext`, `:424`) | Apple-only |
| CommonCrypto | 1 | `Helpers/WIFParser.swift` | Apple-only |
| Combine | 1 | `PlatformWallet/PlatformWalletManager.swift` (`ObservableObject` + `@Published`, `:396-413`) | Apple-only (OpenCombine exists) |
| Darwin | 1 | `Persistence/DashLegacySchemaBridge.swift` | Glibc/ucrt instead |
| CoreFoundation, Dispatch | 1 each | `Core/Utils/DocumentTypedArray.swift`, `Persistence/DashModelContainer.swift` | available |
| UIKit / AppKit / SwiftUI | **0** | — | — |

There is **no** `#if os(...)` / `#if canImport(...)` anywhere in `Sources` (0 hits). The package links `SystemConfiguration` unconditionally (`Package.swift:28`). Keychain API use (`SecItem*`/`kSec*`): `KeychainManager.swift` 174 refs, `WalletStorage.swift` 98, `KeychainInspector.swift` 35.

### 2.2 Can it compile on Linux/Windows?

**No, not as-is, and not without a real fork.** The package is one target; SwiftData is used by 154 files and — critically — by the central `PlatformWalletManager` (3,232 lines; `ObservableObject` + Combine) and `PlatformWalletPersistenceHandler` (11,573 lines, 206 `ModelContainer`/`ModelContext`/`FetchDescriptor`/`#Predicate` references), which is the only implementation of the Rust persistence vtable (§4). Keys/mnemonics go through Security.framework Keychain. So even a "no-UI" subset would need: a new persistence handler (non-SwiftData), a keychain abstraction (libsecret / Windows Credential Manager / DPAPI), swift-crypto instead of CryptoKit/CommonCrypto, a logging shim for os.log, and Combine removal. Swift on Windows/Linux would compile the remaining pure wrappers.

Rough module classification:

| Kind | Directories (files / lines) | Notes |
|---|---|---|
| Pure FFI wrappers / models (portable after small fixes) | `KeyWallet/` (21 / 4.1k: Wallet, WalletManager, Mnemonic, Account*, AddressPool, Transaction*, KeyDerivation, BLS/EdDSA accounts), `DPP/` (5 / 1.8k), `Models/` (3 / 1.1k), `Address/` (4 / 2.7k), `Voting/` (2 / 0.6k), `Utils/` (8 / 2.4k, except TestnetFaucet's CryptoKit), `Helpers/` (2, WIFParser needs CommonCrypto replacement), `Config/`, `DashNetwork.swift`, `ConcurrencyCompat.swift`, `SDK.swift` (0.9k; os.log + `UserDefaults`) | ~13k lines |
| Wallet engine wrappers (FFI-bound, but entangled with SwiftData/Combine) | `PlatformWallet/` (38 / 33.3k: `PlatformWalletManager*`, `ManagedCoreWallet`, `CoreTransactionBuilder`, `ManagedIdentity`, `IdentityManager`, `DpnsMarketplace`, `TokenActions`, shielded/address/DashPay sync), `FFI/` (7 / 6.6k: KeychainSigner, MnemonicResolverAndPersister, PlatformQueryExtensions, StateTransitionExtensions) | Needs refactor to drop SwiftData + Keychain |
| Persistence (Apple-only) | `Persistence/` (151 / 20.1k: 36 `@Model` types, ~110 frozen schema snapshots V1/V2/SnapshotV3, `DashModelContainer`, legacy DashSync CoreData/SQLite bridge), `Services/DataManager.swift` | Replace wholesale off-Apple |
| Secrets (Apple-only) | `Security/` (3 / 1.4k), `Core/Wallet/WalletStorage.swift` | Replace off-Apple |
| UI | none in the package (UI lives in SwiftExampleApp and the apps) | — |

**On macOS the package is fully usable as-is** (macOS 15 platform declared; no UIKit). Caveat from §6/§4: `WalletStorage`/`KeychainManager` never set `kSecUseDataProtectionKeychain`, so on macOS items land in the legacy file-based login keychain and the `kSecAttrAccessibleWhenUnlockedThisDeviceOnly` semantics differ from iOS (UNVERIFIED behaviour; test before shipping).

---

## 3. Feature surface

Legend: **KW** = rust-dashcore `key-wallet` (pinned rev, paths relative to the rust-dashcore root), **KWM** = `key-wallet-manager`, **SPV** = `dash-spv`, **PW** = `packages/rs-platform-wallet/src`, **PWF** = `packages/rs-platform-wallet-ffi/src`, **SDKF** = `packages/rs-sdk-ffi/src`, **Swift** = `packages/swift-sdk/Sources/SwiftDashSDK`. "In iOS FFI" = reachable from the shipped xcframework.

The important structural point: the iOS stack drives L1 through **platform-wallet** (`PWF` `core_wallet_*` / `platform_wallet_manager_spv_*`), which is richer than raw `key-wallet-ffi` (e.g. coin control and selection strategy exist in PWF but not in key-wallet-ffi's `wallet_build_and_sign_transaction`, which is BIP44-only with hard-coded BranchAndBound, `key-wallet-ffi/src/transaction.rs:88,139-144`).

| Feature | Present? | Where | Maturity / notes |
|---|---|---|---|
| **BIP39 create/restore** | Yes | KW `key-wallet/src/mnemonic.rs:21` (10 languages), `generate` :162, `from_phrase` :218, `to_seed(passphrase)` :267. PWF `platform_wallet_manager_create_wallet_from_mnemonic[_with_birth_height]` (`manager.rs:599,630`), `…_from_seed[_with_birth_height]` (:538,571); `mnemonic_words.rs` (word list, normalize, cleanup). Swift `PlatformWalletManager.swift:225,1351,1559`; `KeyWallet/Mnemonic.swift`. | Mature, heavily tested (~1,245 KW tests). **BIP39 passphrase caveat:** `Wallet::from_mnemonic` hard-codes `to_seed("")` (`key-wallet/src/wallet/initialization.rs:220`); a passphrase requires seed-based creation (`mnemonic_to_seed` + `…_from_seed`), and then the wallet is `WalletType::Seed`. Birth height drives checkpoint selection. |
| **BIP44 / multiple accounts** | Yes | KW `account/account_type.rs:34` `AccountType`: `Standard{BIP44Account\|BIP32Account}` :36, `CoinJoin` :43 (m/9'/c'/4'/a'), `IdentityRegistration` :48, `IdentityTopUp` :50, `IdentityTopUpNotBoundToIdentity` :55, `IdentityInvitation` :57, `AssetLockAddressTopUp` :60, `AssetLockShieldedAddressTopUp` :63, `ProviderVotingKeys` :66, `ProviderOwnerKeys` :69, `ProviderOperatorKeys` :72 (BLS), `ProviderPlatformKeys` :75 (Ed25519), `DashpayReceivingFunds` :78, `DashpayExternalAccount` :88, `PlatformPayment` :99. `Wallet::add_account` (`wallet/accounts.rs:30`). PWF `platform_wallet_manager_get_account_balances` (`wallet.rs:159`). | Mature. Gap limits 30/30, CoinJoin 100, special 5, DIP-17 20 (`gap_limit.rs:14-52`). Multiple **wallets** per manager also supported (`platform_wallet_manager_remove_wallet`, `manager.rs:811`). |
| **SPV: headers** | Yes | SPV `sync/block_headers/`, headers2 (`network/handshake.rs:189`), checkpoints `chain/checkpoints.rs:61,150,170`. Exposed via PWF `platform_wallet_manager_spv_start` (`spv.rs:408`: data_dir, network, user_agent, peers[], restrict_to_configured_peers, start_from_height, devnet name/LLMQ params), `_spv_stop` :583, `_sync_progress` :140, `_spv_connected_peers` :292, `_spv_is_running` :349, `_spv_tip_unix_seconds` :372, `_spv_rescan_filters` :627, `_spv_clear_storage` :655. | Mature (~1,273 SPV tests incl. dashd integration suites). Masternode sync is **forced on** (`spv.rs:385-395`) because asset-lock proofs need ChainLock/ISLock managers. |
| **SPV: BIP157/158 filters** | Yes | `dash/src/bip158.rs:100`, SPV `sync/filter_headers/manager.rs`, `sync/filters/manager.rs`, matching in KWM `matching.rs:39` (rayon). | Mature. Filters are the only wallet-scan path (mempool via `FetchAll` or BIP37 bloom, `client/config.rs:17`). |
| **SPV: masternode lists / quorums** | Yes | SPV `sync/masternodes/manager.rs` (QRInfo + MnListDiff), `dash/src/sml/masternode_list_engine/mod.rs:242`. PWF `platform_wallet_manager_list_masternodes[_v2]` (`wallet.rs:240,288`), `_masternodes_by_voting_key` (`spv.rs:175`). | Mature; recent fix for rejected QRInfo recovery (rust-dashcore #947). |
| **ChainLocks / InstantSend** | Yes (verified with BLS) | SPV `sync/chainlock/manager.rs:227,248` → `verify_chain_lock` (`message_request_verification.rs:338`); `sync/instantsend/manager.rs:204` → `verify_is_lock` (:183). | Mature. Locks arriving before the MN list is ready are queued/re-checked. |
| **Tx build / sign / broadcast** | Yes | KW `wallet/managed_wallet_info/transaction_builder.rs:78`. PWF `core_wallet/transaction_builder.rs`: `core_wallet_tx_builder_new` :567, `_add_output` :581, `_add_op_return` :615, `_set_change_address` :663, `_preserve_output_order` :695, `_change_to_first_input` :712, `_set_fee_rate` :727, `_set_selection_strategy` :842, `_set_special_payload` :885, `_finalize` :124 (atomic select+reserve+sign). Broadcast `core_wallet/broadcast.rs:57` (`core_wallet_broadcast_signed_transaction`), `_abandon_signed_transaction` :134. Swift `PlatformWallet/CoreWallet/CoreTransactionBuilder.swift:192-373`. | Mature. Production broadcaster is **pure P2P** `SpvBroadcaster` with acceptance detection (withheld-peer echo / ISLock / confirmation); `DapiBroadcaster` is a fallback for SPV-less wallets (`PW/broadcaster.rs:1-13,107,219`). UTXO **reservations** prevent double-selection across concurrent builds (`PW/wallet/reservations.rs`). |
| **Coin control / selected inputs** | Yes | PWF `core_wallet_tx_builder_add_inputs_from_outpoints` (`transaction_builder.rs:935`) + `core_wallet_tx_builder_use_only_added_inputs` (:758); strategies `CoreSelectionStrategyFFI {SmallestFirst, LargestFirst, BranchAndBound, OptimalConsolidation, Random, All}` (:506). KW `coin_selection.rs:86`. | Works; `Random` is not actually random (TODO `coin_selection.rs:323`). iOS uses it only for CrowdNode single-address spends. No UTXO-freeze/lock list UI primitive beyond reservations (UNVERIFIED). |
| **Fee estimation** | Static only | KW `fee.rs:12` `FeeRate` sat/kB: economy 500, normal/min 1000, priority 2000 (:67-92); size estimates :96,:115. PWF `core_wallet_signed_transaction_fee` (`broadcast.rs:185`), `core_wallet_pooled_max_sendable` (`transaction_builder.rs:817`). | **No network fee estimation** (SPV has none; `feefilter` parsed but unused, `dash-spv/src/network/message_type.rs:112`). Fine for Dash's flat 1 duff/byte market. |
| **CoinJoin mixing** | **No** (account + detection only) | KW `AccountType::CoinJoin`, `account/coinjoin.rs:15` `CoinJoinPools`, heuristic classifier `transaction_router/mod.rs:179-212`, CoinJoin gap discovery during filter sync (`sync/filters/manager.rs:1283,1446`). Only P2P msg: `SendDsq(bool)` (`dash/src/network/message.rs:275`). PWF `platform_wallet_manager_shielded_fund_from_asset_lock_coinjoin_drain`. | No dsa/dsq/dsi/dsf/dss/dssu/dsc/dstx, no session, no mixing client. iOS only reads CoinJoin balance and sweeps CoinJoin funds out. **A dash-qt replacement would need a CoinJoin client written from scratch.** |
| **Governance (proposals, votes)** | **No** | Nothing in KW/SPV/PW (only comments, e.g. `PW/masternode/locator.rs:66`). | Must be built (gobject sync/vote P2P messages, or via a trusted RPC/Insight). The Platform-side "voting" that exists is DPNS **contested-name** voting (below), not Core governance. |
| **Masternode / ProTx** | Partial | Parsing: all ProTx payloads (`dash/src/blockdata/transaction/special_transaction/mod.rs:70`; `provider_registration.rs:99`, `provider_update_service.rs:64`, `provider_update_registrar.rs:49`, `provider_update_revocation.rs:55`); FFI decode `transaction_decode` (`key-wallet-ffi/src/tx_decode.rs:135`). Key derivation DIP-3 voting/owner/operator(BLS)/platform(Ed25519) (`account_type.rs:493-534`); PWF `platform_wallet_provider_key_at_index` (`provider_key_at_index.rs:146`), `platform_wallet_platform_node_id_from_ed25519_pubkey` (:378). **ProUpServTx (revive/unban)**: PW `masternode/update_service.rs` (operator-BLS-signed, revive-only), PWF `platform_wallet_manager_masternode_update_service` & `_prepare_update_service` (`masternode_update_service.rs:304-487`). Locate/verify keys: `masternode_locator.rs:147,254`. Tracked (non-wallet) masternodes: `tracked_masternode.rs:75-279`. Evonode credit withdrawal: `masternode_withdrawal.rs:120,195`. | **No ProRegTx / ProUpRegTx / ProUpRevTx builders** — only generic `set_special_payload` + payload finalizer. No collateral-lock UI primitive (UNVERIFIED). |
| **Platform identities** | Yes | PWF `identity_registration_funded_with_signer.rs` (`platform_wallet_register_identity_with_funding_signer`, resume/top-up with existing asset lock), `identity_registration_with_signer.rs`, `identity_top_up.rs`, `identity_update.rs`, `identity_transfer.rs`, `identity_withdrawal.rs`, `identity_discovery.rs`, `identity_sync.rs` (13 fns), `asset_lock/*` (build, proof, resume, recover). SDKF `identity/` (40 fns). | Mature, used in production iOS (TestFlight). Funding from L1 asset lock, from Platform addresses, or from the shielded pool. |
| **DPNS usernames** | Yes | PWF `dpns.rs` (`platform_wallet_register_dpns_name_with_signer`, resolve, search, sync, contested names, contest vote state), `dpns_marketplace.rs` (15 fns: search, set price, delist, transfer, purchase, history). SDKF `dpns/` (17 fns), `contested_resource/` incl. `dash_sdk_contested_resource_cast_vote`. | Mature. Masternode contested-name voting supported (iOS `MasternodeVoteCaster`). |
| **DashPay contacts** | Yes | PWF `dashpay.rs` (16: sync/send/accept contact requests, QR auto-accept, ignore, `platform_wallet_send_dashpay_payment`), `contact_request.rs`, `established_contact.rs`, `dashpay_profile.rs`, `dashpay_sync.rs`, `invitation.rs` (create/claim/parse invitations). PW `wallet/identity/crypto/{dip14,contact_info,invitation}.rs`. | Mature. DIP-15 avatar hashing pulls the `image` crate. |
| **Credits: top-up / withdrawals / Platform addresses** | Yes | Identity: `identity_top_up.rs`, `identity_withdrawal.rs` (`platform_wallet_withdraw_credits_with_signer`), `identity_transfer.rs`. Platform (DIP-17) addresses: `platform_addresses/{wallet,sync,transfer,withdrawal,fund_from_asset_lock,funding_fee}.rs`, `platform_address_sync.rs`. | Mature; iOS "BLAST" address sync. |
| **Tokens** | Yes | PWF `tokens/` (mint, burn, transfer, freeze/unfreeze, destroy frozen, pause/resume, claim, set price, purchase, update config, group-action queries). SDKF `token/` (24 fns, queries). Swift `PlatformWallet/Tokens/TokenActions.swift`. | Mature. Token shielded pools added on v5.0-dev only (#4760). |
| **Shielded pool (Orchard)** | Yes (feature `shielded`) | PW `wallet/shielded/*` (coordinator, prover, note selection, sync, file store). PWF `shielded_sync.rs` (14) + `shielded_send.rs` (19): shield / unshield / transfer / withdraw / shield-to-recipient / identity create & top-up from pool / fund from asset lock / prover warm-up / fee estimate. | Newest, most actively changing area; heavy deps (halo2, orchard, grovedb-commitment-tree). Proving is CPU-heavy (`platform_wallet_shielded_warm_up_prover`). |
| **Address book** | No (labels only) | KW `AddressInfo.label` (`managed_account/address_pool.rs:239-261`), `TransactionRecord.label` (`transaction_record.rs:17,100,170`). DashPay contacts are the only "contacts". | App must own a payee address book. |
| **Watch-only** | Yes (internal), limited FFI | KW `Wallet::new_watch_only`/`from_xpub` (`initialization.rs:178,261`), `WalletType::{WatchOnly, ExternalSignable}` (`wallet/mod.rs:37`). PW **always** runs wallets as external-signable after registration (`manager/wallet_lifecycle.rs:375`) and restores from persisted xpubs (`PWF manager.rs:651-668`). | No first-class "import an xpub as a watch-only wallet" API in PWF (UNVERIFIED — not found); key-wallet-ffi only via `wallet_add_account_with_string_xpub` / serialized-bytes import. |
| **BIP21 URIs / BIP70** | BIP21 no; BIP70 transport-agnostic hooks only | `Address::to_qr_uri()` only (`dash/src/address.rs:1202`); `dash:` stripping in DashPay QR (`PW/wallet/identity/crypto/auto_accept.rs:348`). BIP70/BIP270 "sign now, submit on merchant ack": PW `wallet/signed_payment_registry.rs`, PWF `core_wallet_signed_payment_finalize[_with_deliverable]` (`transaction_builder.rs:265,473`), `core_wallet_signed_payment_broadcast/_release` (`signed_payment.rs:76,146`). | URI parsing and BIP70 protobuf/HTTP are app code on iOS (`BIP70PaymentService+App`, `PaymentNetworkResolver`). |
| **Message signing / verification** | Sign: yes; verify: Rust only | Sign: PW `wallet/core/sign_message.rs:137` (Core `signmessage`-compatible, base64), PWF `core_wallet_sign_message` (`core_wallet/sign_message.rs:100`), Swift `ManagedCoreWallet.swift:192`. Verify: `dash/src/sign_message.rs` (`is_signed_by_address` :166, `recover_pubkey` :150). | No `verifymessage` FFI export — trivial to add. |
| **Wallet encryption / backup / export** | No at-rest encryption in the engine | KW `wallet/backup.rs:29,50` (bincode, **plaintext** mnemonic/xprv); BIP38 single-key only (`key-wallet-ffi/src/bip38.rs:17,35`). Rust holds no long-term seed (external-signable + mnemonic resolver). `rs-platform-wallet-storage` has an Argon2id + XChaCha20-Poly1305 vault and OS-keyring backends but is unused by any FFI (§4). | On iOS protection = Keychain `WhenUnlockedThisDeviceOnly`, no PIN encryption. Export = show mnemonic (`SeedBackupView`). Desktop needs its own passphrase-encrypted secret store and a backup story; dash-qt `wallet.dat` import is out of scope of this stack. |
| **Multisig / PSBT** | No (PSBT type exists, unused) | KW `psbt/mod.rs:49` (sign/combine/extract, tested); `Address::p2sh` only. | No multisig wallet, no PSBT FFI. |
| **Hardware wallets** | No | Integration seam: async `Signer` trait (`key-wallet/src/signer.rs:82,139`) + `WalletType::ExternalSignable` + `build_and_sign_transaction_with_signer` (`transaction_building.rs:140`); rs-sdk-ffi signer vtable for Platform keys. | No Ledger/Trezor/HWI code. |
| **Platform queries, documents, contracts** | Yes | SDKF `document/` (26), `data_contract/` (13, incl. v5.0 propertyConstraints), `system/`, `protocol_version/`, `evonode/`, `group/`, `voting/`. | Mature. |

---

## 4. Persistence model

### 4.1 Overview

| Data | Store | Owner | Where defined |
|---|---|---|---|
| Wallet state (accounts, address pools, txs, UTXOs, ISLocks, sync heights), identities, keys (public), contacts, asset locks, Platform addresses, tokens, DPNS, invitations, DashPay, shielded notes/nullifiers | **Host-side** store fed by Rust changesets through a C callback vtable. iOS: **SwiftData** (one `DashModel.sqlite` per network in the app). Android: Room. | Host | PW `changeset/traits.rs:232` (trait), PWF `persistence.rs:600` (vtable) |
| SPV chain data (headers, filter headers, filters, blocks, MN state, peers) | Flat files under the `data_dir` passed to `platform_wallet_manager_spv_start` | dash-spv | `dash-spv/src/storage/` |
| Shielded commitment tree | One SQLite file per network (bundled rusqlite, WAL) | PW | `PW/wallet/shielded/file_store.rs:79` |
| Mnemonic, identity private keys | Apple Keychain (iOS); Android Keystore-wrapped DataStore | Host (Swift `WalletStorage`/`KeychainManager`) | §4.5 |
| Unused alternative | `rs-platform-wallet-storage`: Rust SQLite persister + keyring/encrypted-file vault | — | §4.4 |

### 4.2 Rust → host persistence contract

- Trait `PlatformWalletPersistence` (`PW/changeset/traits.rs:232`): required `store(wallet_id, PlatformWalletChangeSet)` :314, `flush` :359, `load() -> ClientStartState` :404; optional `persist/load_tracked_masternodes` :370/:381, `get_core_tx_record` :445, `list_wallet_core_txids` :480, `get_dpns_name_state` :578; capability flags `store_commits_inline`, `persistence_capabilities`, `persists_durably` (:239-280). Wrapper `PW/wallet/persister.rs:56`.
- `PlatformWalletChangeSet` (`PW/changeset/changeset.rs:2104`) — optional deltas: `core`, `identities`, `identity_keys`, `contacts`, `platform_addresses`, `asset_locks`, `invitations`, `dpns_name_states`, `token_balances`, `dashpay_profiles`, `dashpay_payments_overlay`, `wallet_metadata`, `identity_scan_state`, `account_registrations`, `provider_key_account_registrations`, `account_address_pools`, `pending_contact_crypto_*`, `shielded` (:2160-2197). Merge rules in `changeset/merge.rs`.
- C vtable `PersistenceCallbacks` (`PWF/persistence.rs:600`): transactional bracket `on_changeset_begin_fn`/`on_changeset_end_fn(success)` (:608/:626), `on_store_fn`, `on_flush_fn`, ~13 `on_persist_*` (address balances, wallet changeset, sync state, account registrations, wallet metadata, address pools, identities, identity keys, token balances, contacts, asset locks, invitations, DashPay payments), 6 shielded persist callbacks, load callbacks each with a `_free_fn` (`on_load_wallet_list_fn`, shielded loads, `on_get_core_tx_record_fn`, `on_list_wallet_core_txids_fn`), `release_fn` (:1153). Versioned `PersistenceCallbacksExtension` (:286-400) adds DPNS name states, tracked masternodes, sweeps / chain-lock height / UTXO verdicts, identity balance block time. `FFIPersister` impl at :1351/:1636. Known gap: `pending_contact_crypto` has no slot (:1647-1652).
- Load: `platform_wallet_manager_load_from_persistor` (`PWF/manager.rs:651-668`) rebuilds every wallet **watch-only/external-signable** from stored root + account xpubs; private material is only ever reached through the mnemonic resolver.
- **A desktop host must implement this vtable** (iOS: 11,573-line `PlatformWalletPersistenceHandler.swift`; Android: 4,541-line `PlatformWalletPersistenceHandler.kt` over Room v14 with 34 entities, `kotlin-sdk/.../DashDatabase.kt:165-166`, bridged by `rs-unified-sdk-jni/src/persistence.rs:123` `build_vtable`) — or bypass FFI and use `rs-platform-wallet-storage` from Rust.

### 4.3 Swift / SwiftData side (`Swift/Persistence`, 151 files)

- 36 `@Model` types (`Persistence/Models/`): wallet/core (PersistentWallet, PersistentAccount, PersistentCoreAddress, PersistentTransaction, PersistentTxo, PersistentPendingInput, PersistentWalletManagerMetadata, PersistentAssetLock), Platform addresses (PersistentPlatformAddress, …SyncState), identity (PersistentIdentity, PersistentPublicKey, PersistentIdentityBalanceMetadata, PersistentDPNSName), DashPay (Profile, ContactProfile, ContactRequest, Payment, IgnoredSender, Invitation), tokens (Token, TokenBalance, TokenHistoryEvent), contracts/documents (DataContract, DocumentType, Document, Index, Property, Keyword), shielded (Note, OutgoingNote, SyncState, Activity, ViewingKey), masternodes (Masternode, TrackedMasternode).
- `DashModelContainer.swift`: live schema `DashSchemaV3` v3.0.0 (:103, :377); frozen `DashSchemaV1` (:319), `DashSchemaV2` (:332) and `DashSchemaSnapshotV3` in `Persistence/FrozenSchemas/` (35 + 36 + 38 files); migration plans `DashMigrationPlan` (V2→V3, :288-296) and `DashAcceptedV1MigrationPlan` (:302-306), routed by store metadata hashes (`migrationPlan(at:)` :213-271). `create(url:)` :135 / `createAsync(url:)` :152; default `create()` :116 uses SwiftData's default location. The schema is frozen per App Store release (`SCHEMA_RELEASES.md`, `schema-releases.json`) — a desktop app sharing this package inherits that migration discipline.
- `PlatformWalletPersistenceHandler.swift`: retained context pointer released by `release_fn` (:3215-3222), `makeCallbacks()` (:3223-3263), one background `ModelContext` (autosave off, serial queue, :330,:344); `beginChangeset` (:3278) / `endChangeset(success:)` (:3340) map one Rust `store()` to one SwiftData `save()`/`rollback()`.
- `DashLegacySchemaBridge` / `DashLegacyStoreSQLite`: bridge for the SDK's **own** early SwiftData 1.0.0 stores (raw SQLite3 backup + candidate promotion), **not** DashSync. DashSync migration lives in the iOS app (`SwiftDashSDKKeyMigrator`, §6).

### 4.4 `rs-platform-wallet-storage` (crate `platform-wallet-storage`) — present but unwired

- Manifest `packages/rs-platform-wallet-storage/Cargo.toml:2,8`: "SQLite persistence and keyring_core secret backends (encrypted-file + OS keyring)"; default features `sqlite, cli, secrets, kv` (:221), optional `shielded` (:319).
- `SqlitePersister` (`src/sqlite/persister.rs:226`, `open` :280, **`impl PlatformWalletPersistence`** :1301): one DB for many wallets, WAL, online backup/restore, strict/recovery load. Migrations via `refinery` — 18 migrations `migrations/V001__initial.rs` … `V018__identity_hard_delete.rs`; V001 tables include `wallet_metadata`, `account_registrations`, `account_address_pools`, `core_transactions`, `core_utxos`, `core_instant_locks`, `core_derived_addresses`, `core_sync_state`, `identities`, `identity_keys`, `contacts`, `platform_addresses`, `asset_locks`, `token_balances`, `dashpay_profiles`, …; later: invitations, DPNS name states, tracked masternodes, shielded viewing keys, identity scan state. Maintenance CLI `src/bin/platform-wallet-storage.rs`.
- Secrets: Argon2id + XChaCha20-Poly1305 encrypted-file vault, plus `keyring-core` OS backends: apple-native keychain, **dbus Secret Service (Linux)**, **windows-native** (Cargo.toml:87-161); `memsec`. Rule: no private keys in SQLite (`SECRETS.md:25-41`).
- **No consumer in the platform tree**: no other `Cargo.toml` depends on it (root `Cargo.toml:49` lists it as a member only); not linked by platform-wallet-ffi, rs-unified-sdk-ffi, JNI, Swift or Kotlin. Likely intended for Rust hosts such as dash-evo-tool (UNVERIFIED). **For a desktop wallet this is the most direct cross-platform persistence + secret-storage path** — either from a Rust-native app, or by adding a thin FFI (`platform_wallet_manager_create_with_sqlite(path)`) — UNVERIFIED that its schema covers every changeset field the SwiftData handler persists (e.g. shielded notes are behind its `shielded` feature).

### 4.5 Keys and mnemonics (Swift)

- `Core/Wallet/WalletStorage.swift`: `kSecClassGenericPassword`, service `org.dashfoundation.wallet` (env override `DASH_KEYCHAIN_SERVICE`, :50-52); account `wallet.mnemonic.<64-hex walletId>` (:79-82) and `wallet.metadata.<hex>` (:292); `kSecAttrAccessibleWhenUnlockedThisDeviceOnly` (:34,:104,:319); no `SecAccessControl` on reads; biometric variant (:387-431, `LAContext` :424) has no callers; **no PIN encryption** (legacy items removed, :476-498). Doc comment :32-42 (v5.0) states the mnemonic is protected only by "device unlocked" and Rust rebuilds seed/xprv per operation then wipes them.
- `Security/KeychainManager.swift`: same service; identity private keys `identity_privkey.<walletIdHex>.<derivationPath>` (:643), masternode voting/owner/payout keys (:287-343); `WhenUnlockedThisDeviceOnly`, non-synchronizable (:145-146, :650-651).
- `FFI/MnemonicResolverAndPersister.swift`: resolver callback reads the keychain and copies into a Rust `Zeroizing` buffer (:139-160); Swift keeps an XOR-masked copy cleared with `memset_s` (:16-58); persister half (:320) writes derived identity keys back to the keychain.
- `FFI/KeychainSigner.swift`: Platform signing — looks up `PersistentPublicKey` (SwiftData) → keychain private key → `dash_sdk_signer_create_from_private_key`, zeroes after; falls back to deriving via the mnemonic resolver (:616, :881).
- Rust keeps no long-term seed: wallets are downgraded with `downgrade_to_external_signable()` after registration (`PW/manager/wallet_lifecycle.rs:375`).
- **macOS caveat:** `kSecUseDataProtectionKeychain` is never set, so on macOS the items go to the legacy file-based login keychain (consistent with `run_tests.sh:7-24` creating a temporary unlocked keychain for CI). Accessibility-class semantics there differ from iOS (UNVERIFIED in practice). A Mac app should opt into the data-protection keychain (requires a signed app with a keychain-access-group entitlement) or wrap secrets with its own passphrase.

---

## 5. SwiftExampleApp

`packages/swift-sdk/SwiftExampleApp` — 159 Swift files in the app target (194 incl. tests), bundle id `org.dashfoundation.DashDeveloperPro` (`project.pbxproj:447`). It is effectively the **feature catalogue / developer console** for the whole SDK.

**What it demonstrates** (paths under `SwiftExampleApp/SwiftExampleApp/`; root `ContentView.swift:6-7,95-140` tabs: Sync, Wallets, Identities, DashPay, Settings):

- Wallet create/restore/backup: `Core/Views/CreateWalletView`, `SeedBackupView`, `WalletDetailView` (reveal gated by `LAContext`, :710-725), `AccountListView`/`AccountDetailView`, `Views/RecoverWalletsSheet`; launch-time recovery of keychain mnemonics lacking a SwiftData row (`ContentView.swift:295ff`).
- SPV: `Core/Services/CoreSpvLauncher` (data dir `Documents/SPV/<network>`, :61-64), `MasternodeSync`, `WalletSyncCoordinatorLifecycle`; `SyncStatusView`.
- L1 send/receive: `Core/Views/SendTransactionView` + `ViewModels/SendViewModel`, `ReceiveAddressView`, `QRScannerView`, `TransactionListView`/`TransactionDetailView`.
- Platform addresses/credits: `Views/TransferPlatformAddressView`, `WithdrawPlatformAddressView`, `FundFromAssetLockPlatformAddressView`, `WithdrawalCoreFeeRates`.
- Identities: `Views/CreateIdentityView`, `LoadIdentityView`, `TopUpIdentityView`, `TransferCreditsView`, `WithdrawCreditsView`, `AddIdentityKeyView`, `KeysListView`, `RegistrationProgressView`.
- DPNS: `DPNSTestView`, `RegisterNameView`, `SelectMainNameView`, `DpnsMarketplaceView`; contests `ContestDetailView`.
- DashPay: `Views/DashPay/*` (contacts, requests, hidden/ignored, profile, payments, invitations create/claim/reclaim).
- Tokens: `TokensView`, `TokenDetailsView`, `Views/TokenActions/*` (mint/burn/transfer/freeze/unfreeze/destroy/pause/resume/claim/set price/purchase/update max supply/co-sign group proposals).
- Shielded: `Core/Views/ShieldedActivityView`, `ShieldedFundFromAssetLockView`, `SeedShieldedPoolView`, `Core/Services/ShieldedService` (tree DB `Documents/shielded_tree_<network>.sqlite`, :1156-1162).
- Contracts/documents/queries: `ContractsTabView`, `DocumentsView`, `CountDocumentsView`, `SumAverageDocumentsView`, `GroveDBPathElementsView`, `PlatformQueriesView`, `StateTransitionsView`.
- Masternodes: `Core/Views/MasternodeDetailView`.
- Browser login over Bluetooth (DashPay Connect prototype): `Views/ShareLoginKeyView`, `Services/BrowserLoginPeripheral` (CoreBluetooth).
- Diagnostics: `OptionsView` data section (:460-473) → `StorageExplorerView`, `KeychainExplorerView`, `WalletMemoryExplorerView`, `BannedAddressesView`; `DiagnosticsView`, `LogExporter`.

**macOS-capable? Not as configured.** `SDKROOT = iphoneos` (`project.pbxproj:361,419`), `IPHONEOS_DEPLOYMENT_TARGET = 18.5`, `TARGETED_DEVICE_FAMILY = "1,2"`, no `SUPPORTS_MACCATALYST` / `SUPPORTED_PLATFORMS` / `MACOSX_DEPLOYMENT_TARGET`. 6 files `import UIKit`; 21 files use UIKit/AVCapture/CoreBluetooth APIs; ~336 iOS-only SwiftUI modifier uses (`navigationBarTitleDisplayMode`, `keyboardType`, `textInputAutocapitalization`, …); only 3 platform conditionals (`EnvLoader.swift:86`, `DiagnosticsView.swift:591` with an `NSPasteboard` `#else`, `LogExporter.swift:241`). Mac Catalyst would additionally need an `ios-macabi` xcframework slice (not built). "Designed for iPad" on Apple-silicon Macs may work by default (UNVERIFIED). A native macOS SwiftUI target is feasible because the views are SwiftUI, but needs the iOS modifiers conditionalised and the scanner/BLE/pasteboard pieces replaced.

**The SDK itself runs on macOS:** `run_tests.sh:146-148` runs `swift test` on the host Mac against the `macos-arm64` slice (offline `SwiftDashSDKTests` with fixture stores; `SwiftDashSDKIntegrationTests` gated by `RUN_INTEGRATION_TESTS=1` / `RUN_TESTNET_TESTS=1`, `Package.swift:31-49`).

---

## 6. How dashwallet-ios uses the SDK

Repo `/Users/pasta/workspace/dashwallet-ios`, `develop` @ `7c0064d6b2` (2026-10-02), clean. Paths below are relative to `DashWallet/Sources/` unless noted.

### 6.1 Dependency wiring

- **Local SPM path, unpinned:** `DashWallet.xcodeproj/project.pbxproj:14054-14056` — `XCLocalSwiftPackageReference relativePath = "../platform/packages/swift-sdk"`; product `SwiftDashSDK` linked into both `dashwallet` and `dashpay` targets (`:9154`, `:9264`). `Package.resolved` pins only DashUIKit. The xcframework is gitignored and built by hand (`CLAUDE.md:41-43`: `./build_ios.sh --target ios --target sim`).
- Release CI: `.github/workflows/release-dashpay-testflight.yml:15-18` defaults `platform_ref` to **`v5.0-dev`**, checks platform out (:212-217) and builds a release xcframework (:417-425). (`CLAUDE.md:75` still says v4.2-dev — stale.) Local dev uses whatever `../platform` has checked out (currently v4.3-dev).
- **DashSync is fully removed** (no pod, no imports; `DS*` names survive only in comments). No feature flag between stacks. The only bridge is `Infrastructure/SwiftDashSDK/SwiftDashSDKKeyMigrator.swift` (:36), which imports DashSync's old `org.dashfoundation.dash` keychain mnemonics into the SDK once and never deletes them, plus launch holds (`LegacyWalletMigrationLaunchHold` :689) and `SwiftDashSDKPhraseRepairer.swift`.
- 123 app files `import SwiftDashSDK` (adapter dir 42, `UI/Menu` 25, `UI/Payments` 13, `UI/DashPay` 11, …) — UI also calls the SDK directly (e.g. `KeychainSigner(modelContainer:)` in `UI/Payments/InternalTransfer/IdentityWithdrawViewModel.swift:135`).

### 6.2 Adapter layer — `Infrastructure/SwiftDashSDK/` (63 files, ~23.9k lines; subdirs `Identity/`, `Contacts/`, `Invitations/`, `Voting/`, `Masternodes/`)

**Runtime / host**
- `SwiftDashSDKHost.swift` — process singleton (`static let shared`, :134) owning `sdk`, `manager: PlatformWalletManager`, `wallet: ManagedPlatformWallet`, `modelContainer`, `runningNetwork` (:148-152). One-time `LoggingPreferences.configure()` + `SDK.initialize()` (:166-190). `start(network:)` (:497) is idempotent; network switch tears down and rebuilds (`buildRuntime` :980). `makeRuntime` (:1034-1125): (1) `SDK(network:platformVersion:)` on a dedicated queue (:1005-1027); (2) `DashModelContainer.createAsync(url:)` at `Documents/SwiftDashSDK/Platform/<scope>/DashModel.sqlite` (:1648-1664), cached per scope (:153); (3) `PlatformWalletManager()` → `.configure(sdk:modelContainer:)` (:1105-1109); (4) `manager.loadFromPersistor()` (:1215). `createOrImportWallet` (:566) / `addWallet` (:721) also provision the same mnemonic on other networks; only the host writes the mnemonic (:837). `stopAsync()` (:942). Separate key-derivation `WalletManager` (`derivationWallet()` :425). Regtest rejected (:1035).
- `SwiftDashSDKWalletRuntime.swift` — lifecycle owner; all operations serialised through `SerialAsyncLifecycleQueue` (:27). Start order host → SPV → BLAST (Platform address sync); stop reverse (header :1-16). API: `startIfReady` (:237), `stop` (:275), `stopCoreSPV`/`restartCoreSPV` (:328/:336), `switchNetwork(to:)` (:452), `switchWallet(to:)` (:569), `performAddWallet` (:605), `handleWalletWiped` (:367), `rotatePeers` (:427); waits for the seed migrator (:1013). Kicked from `AppDelegate.m:185-188`.
- `WalletEnvironment.swift` — network selection persisted as DashSync-compatible `CURRENT_CHAIN_TYPE_KEY` (:26-75), `network: SwiftDashSDK.Network?` (:105), posts `DWCurrentNetworkDidChange`. `WalletLifecycleTransitionState.swift` (one interactive lifecycle op at a time), `WalletPreparationFailure.swift`, `DevnetConfiguration.swift`.

**SPV**
- `SwiftDashSDKSPVCoordinator.swift` — singleton facade; publishes `progress`, `state`, `tipHeight`, `bestPeerHeight`, `lastError`, `syncProgress`, `connectedPeers` (:190-201). Start builds `PlatformSpvStartConfig(dataDir, network, peers, restrictToConfiguredPeers, startFromHeight: 0, devnetName)` → `manager.startSpv(config:)` (:621-654); widens the CoinJoin gap once before start (:632-638, :768); applies pending birth-height resync by deleting the SPV store (:644, :810). Stop `manager.stopSpv()` (:729). Subscribes (Combine) to `manager.$spvProgress` / `$spvPeers`; balance refresh on context save throttled to 1 s (:876-912). Gotcha: fully synced SPV sits in `waitForEvents` (~1.0 progress), not `.synced` (`CLAUDE.md:71`).
- `SwiftDashSDKWalletState.swift` (balances incl. `coinJoinBalanceDuffs` :239), `PlatformAddressSyncCoordinator.swift` (2.3k lines, BLAST Platform address sync + transfers + shielded-recovery monitoring), shielded/Platform balance controllers, `HomeBalancePresentation`.
- No background SPV on iOS; `BackgroundRefreshCoordinator` (BGTaskScheduler) re-runs `startIfReady` on activation/refresh.

**Sending**
- `SwiftDashSDKTransactionSender.swift` — `buildAndSign(address:amount:)` / `buildAndSign(recipients:)` (:93/:100) = `CoreTransactionBuilder(network:)` → `addOutput` → `finalizeAtomic(wallet:accountType: .allSpendable)` (atomic select+reserve+sign; CoinJoin coins excluded, :113-124); `broadcast(_:)` (:571) separate. Fee = builder default; only the CoinJoin sweep fixes 1000 duffs/kB (:48, :303). Coin control used only for CrowdNode (`buildAndSignFromAddress` :379). Also Maya swap deposit with OP_RETURN memo (:147), CoinJoin sweep in ≤500-input chunks (:251), `maxSendFeeReserveDuffs` (:516).
- Flow: `Models/Transactions/WalletSendService.swift` = auth → build/sign → user confirm → broadcast (`AuthenticationGate` :676). Helpers: `BIP70SendAuthorizer`, `BIP70PaymentService+App`, `PaymentNetworkResolver` (BIP70 is app code), `UnconfirmedTransactionRemover` (local surgery — no FFI for removal), receive-address reader/provider, `InsightExplorerAPI` (phrase repair only).

**Identity / DPNS / DashPay / masternodes**
- `Identity/DWIdentityRegistrationCoordinator` (2.1k lines, :256): auth → `prePersistIdentityKeysForRegistration` → `registerIdentityWithFunding` (asset lock → IS/CL → IdentityCreate) → `registerDpnsName`; alternative funding from Platform addresses or shielded pool (`ShieldedIdentityFundingReadiness`); progress by polling SwiftData `PersistentAssetLock` every 0.5 s. Related: `DWCurrentUserIdentityInfo` (1.3k), `DWIdentityKeyUpgrader`, `DWProfileUpdateCoordinator`, `DWContestedNameStatusService`, `UsernameMarketplaceService`, `AssetLockRecoveryService`.
- `Contacts/SwiftDashSDKContactsService` (1.2k), `Invitations/DWInvitationService` (invitee side), `Voting/{ContestedNamesService, MasternodeVoterRegistry, MasternodeVoteCaster}` (DPNS contest voting), `Masternodes/{TrackedMasternodeKeyVault, EvonodeEpochBlocksService/Monitor}`.

**Secrets**
- Mnemonic in SDK `WalletStorage` (§4.5); signing pulls it through `MnemonicResolver` per operation (`CoreTransactionBuilder.swift:332-345`); Platform signing via `KeychainSigner`. App PIN is separate and DashSync-compatible (`Infrastructure/Authentication/PinStore.swift`, `AuthenticationService.swift`).

**App-owned (not SDK) features:** fiat rates (`RatesProvider.swift`), Uphold/Topper/Coinbase/Maya/ZenLedger/Explore (HTTP), CrowdNode (suspended), BIP70, app metadata DB (SQLite.swift, `Infrastructure/Database/DatabaseConnection.swift:84-90`). CoinJoin: balance read + sweep only. Governance menu = masternode tools + DPNS voting; no Core proposal voting found (UNVERIFIED absence).

### 6.3 iOS-only couplings a desktop port must replace

- In the adapter: `PlatformAddressSyncCoordinator.swift:34,374,383-386` (`UIApplication` state + didBecomeActive/didEnterBackground), `Masternodes/EvonodeEpochBlocksMonitor.swift:23,135`, `Identity/DWProfileUpdateBridge.swift:26,68` (`UIImage`).
- Around it: `BackgroundRefreshCoordinator` (BackgroundTasks), `SyncingActivityMonitor.swift:174` (idle timer), `AuthenticationService.swift:103` (lock on background), Obj-C `AppDelegate.m:185-188` startup, many `@objc`/`NSObject` bridges (`DWSwiftDashSDK*`, `DW*Bridge`) that a Swift-only app can drop.
- Paths: `.documentDirectory` for SDK stores (`SwiftDashSDKHost.swift:1649`, `SwiftDashSDKSPVCoordinator.swift:1059`) — on macOS use Application Support.

### 6.4 Shape to copy for desktop

```
AppRuntime (≈ SwiftDashSDKWalletRuntime: serial lifecycle queue, start/stop/switch network/switch wallet)
  └─ Host (≈ SwiftDashSDKHost): SDK(trusted) + store container per network + PlatformWalletManager(+persistence handler) + loadFromPersistor
       ├─ SPVCoordinator: PlatformSpvStartConfig(dataDir,…) → startSpv/stopSpv, progress & peers streams
       ├─ WalletState: balances (spendable / CoinJoin / Platform / shielded)
       ├─ TransactionSender: CoreTransactionBuilder → finalizeAtomic → confirm → broadcast
       ├─ PlatformAddressSync (BLAST), Shielded controllers
       └─ Identity/DPNS/DashPay/Masternode coordinators
  Secrets: WalletStorage (mnemonic) + KeychainManager (identity keys) + MnemonicResolver/KeychainSigner callbacks
```

---

## 7. C headers / non-Swift consumers; rs-unified-sdk-ffi scope

- All four shipped FFI crates generate **plain C** headers with cbindgen (`language = "C"`, `cpp_compat = true` for rs-sdk-ffi, platform-wallet-ffi, dash-spv-ffi): `packages/rs-sdk-ffi/cbindgen.toml` (include guard `DASH_SDK_FFI_H`, exports `dash_sdk_*`, `dash_core_*`, `dash_unified_sdk_*`, excludes key-wallet account enums to avoid duplicate definitions), `packages/rs-platform-wallet-ffi/cbindgen.toml` (`PLATFORM_WALLET_FFI_H`, `ScreamingSnakeCase` enum variants prefixed with the type name), rust-dashcore `key-wallet-ffi/cbindgen.toml` (`KEY_WALLET_FFI_H`, `#include "../dash-network/dash-network.h"`), `dash-network/cbindgen.toml` (`DASH_NETWORK_H`). The umbrella `DashSDKFFI.h` is generated by `build_ios.sh` (`:219-226`) and is trivially reproducible.
- Consequently a non-Swift UI (C++/Qt, Rust, C#, Kotlin/JVM via JNA/Panama, etc.) can consume the same archive + headers. Patterns the host must implement (all plain C function-pointer vtables, defined in the headers):
  - **Persistence callbacks** (`rs-platform-wallet-ffi/src/persistence.rs`, passed to `platform_wallet_manager_create_with_*`): the host stores changesets and returns them on load (`platform_wallet_manager_load_from_persistor`, `manager.rs:651-668`, rebuilds wallets **watch-only** from stored root/account xpubs).
  - **Mnemonic resolver** (`rs-sdk-ffi/src/mnemonic_resolver.rs`: `dash_sdk_mnemonic_resolver_create/_destroy`) — synchronous "give me the BIP-39 phrase for wallet_id" callback; Rust derives and signs, then zeroizes (1024-byte buffer, `MNEMONIC_RESOLVER_BUFFER_CAPACITY`). Design rule: "no mnemonic round-tripping" — derivation lives in Rust, host only reads/writes secrets.
  - **Signer vtable** (`rs-sdk-ffi/src/signer.rs`: `dash_sdk_signer_create[_with_ctx]`, async completion `dash_sdk_sign_async_completion`) for identity-key signing (keychain-backed on iOS).
  - **Event handler** (`platform-wallet-ffi/src/event_handler.rs`) for SPV progress / wallet events.
  - Context-provider callbacks (`rs-sdk-ffi/src/context_provider.rs`) if not using the trusted HTTP provider.
- **rs-unified-sdk-ffi scope**: only an aggregation crate (4 `pub use` lines). It adds no symbols of its own. It deliberately excludes `dash-spv-ffi`. `packages/rs-sdk-ffi/UNIFIED_SDK_ARCHITECTURE.md` describes an older layout ("DashUnifiedSDK.xcframework", `librs_sdk_ffi.a`, 29.5 MB) and is **stale**.
- **Alternative for a Rust-native desktop UI:** skip the C ABI entirely and depend on `platform-wallet` (+ `dash-sdk`, `key-wallet`, `dash-spv`) as Rust crates. Then persistence can use the Rust-native `rs-platform-wallet-storage` crate (see §4) instead of re-implementing the 46-slot persistence vtable, and the FFI/handle layer is unnecessary. This diverges from "built like the iOS app" at the binding layer but keeps the same engine.
- The JNI shim (`rs-unified-sdk-jni`, ~262 `Java_*` fns across `wallet_manager.rs`, `transactions.rs`, `identity.rs`, `dashpay.rs`, `tokens.rs`, `persistence.rs`, …) shows a second host binding of the same engine; `packages/kotlin-sdk/PARITY.md` tracks Swift↔Kotlin parity and is a useful checklist of what a third (desktop) host must implement.
