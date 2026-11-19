# Upgrade-path fixtures

Each `<tag>/` is a data root written by the released Linux app of that tag, and what that release showed for it.
`rust/crates/dw-app/src/upgrade_path_tests.rs` opens a copy of every one with the current build, twice, and
requires the same answers (it runs in `cargo test --workspace`, so in "Rust tests (Linux)" and the macOS job).

- `<tag>/data/`: the data root, byte for byte as the release left it after quitting cleanly (SIGTERM, its engine
  shutdown), less `instance.lock`. Nothing in it was written by any other build.
- `<tag>/expected.json`: the release asset and its SHA-256, the vault passphrase, the two wallets' BIP39 test
  vectors and ids, dashd's view (height, funding and payment txids), the release's answers to 46 read commands
  after a restart (`calls`), and the message signatures it made (`signatures`, each verified by dashd). Paths
  under the data root read `$DATA_ROOT`.

Fixture data only: the wallets are the public BIP39 vectors `abandon … about` and `legal winner … yellow` with
passphrase `TREZOR`, on a private regtest chain that existed for the length of the run. No address in it holds
anything anywhere else.

## Writing one

```sh
cd apps/desktop
DASHD=<dir>/dashd scripts/upgrade-fixture.sh v0.2.0
```

The script downloads the tag's `dash-wallet-<version>-linux-amd64.deb` and `SHA256SUMS` into a new empty
directory, refuses a .deb that does not match, unpacks it into another and runs only its `usr/bin/dash-wallet`.
`e2e-tauri/upgrade-fixture.ts` drives that binary through tauri-driver under a private X display and session bus
(nothing appears on a screen) against a fresh dashd regtest node:

1. an encrypted vault; wallet "Fixture main" (`abandon … about`) and "Fixture second" (`legal winner … yellow`,
   BIP39 passphrase `TREZOR`), both from birth height 0;
2. dashd pays the main wallet 10 and 2.5 tDASH and the second 3 tDASH; the app pays a dashd address 1.25 tDASH
   (with a recipient label, which also files it in the address book); three blocks;
3. a transaction label, an address-book entry, a receive request with amount, label and message; username in
   requests off; dust protection 12345 duffs; backups kept 7; the saved network choice testnet; CoinJoin with
   multi-session, 3 sessions, 6 rounds, 25 DASH; theme dark, unit mDASH, 4 decimals, hidden balance, auto-lock
   900 s, subtract fee by default, masternodes shown;
4. the app quits, opens again offline (`--no-sync`), is unlocked, and its answers are recorded and checked against
   dashd; it quits again and the data root is copied here.

A new release is covered by running the script for its tag and committing the directory; the test needs no change
unless the release renamed a field of a read command's answer (`RENAMED` / `DROPPED` in the test, with the reason).

| Tag | Asset | SHA-256 | Written | dashd |
|---|---|---|---|---|
| v0.1.0 | `dash-wallet-0.1.0-linux-amd64.deb` | `1ac5c6690fc8394935fd62b7ff5b134d2206d8a856943553af264144f0b4b6a1` | 2026-11-19 | 24.0.0 |
