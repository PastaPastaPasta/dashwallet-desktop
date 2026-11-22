# Upgrade-path fixtures

Each `<tag>/` is a data root written by the released Linux app of that tag, and what that release showed for it.
`rust/crates/dw-app/src/upgrade_path_tests.rs` opens a copy of every one with the current build, twice, and
requires the same answers (it runs in `cargo test --workspace`, so in "Rust tests (Linux)" and the macOS job).

- `<tag>/data/`: the data root, byte for byte as the release left it after quitting cleanly (SIGTERM, its engine
  shutdown), less `instance.lock`. Nothing in it was written by any other build. The one file the writer itself
  puts there is the first `settings.json` (the pre-release notice dismissed), before the release first starts;
  the release rewrites it when the settings are saved.
- `<tag>/expected.json`: the release asset and its SHA-256, the vault passphrase, the two wallets' BIP39 test
  vectors and ids, dashd's view (height, funding and payment txids), the release's answers to the read commands
  after a restart (`calls`), and the message signatures it made (`signatures`, one per wallet by an unused
  receive address, each verified by dashd). Paths under the data root read `$DATA_ROOT`. From v0.2.0 also:
  `fundedSignatures`, one per distinct address holding a coin (change included) in the two wallets that have
  keys, so the keys that hold the funds are shown to sign (the watch-only wallet cannot sign); `dashd.deposits` and `dashd.payment.feeDuffs`, dashd's fee for the payment read
  from its mempool, which the recorded balances must equal to the duff; `watchOnly` and `lockedOutpoints`.

Fixture data only: the wallets are the public BIP39 vectors `abandon … about` and `legal winner … yellow` with
passphrase `TREZOR`, on a private regtest chain that existed for the length of the run. No address in it holds
anything anywhere else.

## Writing one

```sh
cd apps/desktop
DASHD=<dir>/dashd scripts/upgrade-fixture.sh v0.2.0 <sha256 of dash-wallet-0.2.0-linux-amd64.deb>
```

The script downloads the tag's `dash-wallet-<version>-linux-amd64.deb` and `SHA256SUMS` into a new empty
directory and refuses a .deb whose SHA-256 is not the one the caller pinned (second argument or
`DW_UPGRADE_SHA256`), is not in `SHA256SUMS`, or has no build attestation from `release.yml` at the tag
(`gh attestation verify --signer-workflow … --source-ref refs/tags/<tag>`, see `docs/release/verifying.md`). It
unpacks it into another directory and runs only its `usr/bin/dash-wallet`. The pinned value is the one in the
table below for a tag already captured; for a new tag, take it from a copy you verified as
`docs/release/verifying.md` says, before the run, and add it to the table with the fixture.
`e2e-tauri/upgrade-fixture.ts` drives that binary through tauri-driver under a private X display and session bus
(nothing appears on a screen) against a fresh dashd regtest node. Each of its two launches picks free loopback
ports for tauri-driver and WebKitWebDriver, refuses to start if either already answers, and sends its first request
(`GET /status`) only once every listener on both ports belongs to the driver it spawned. That check and the session request are
separate steps, so it catches a foreign listener that is already there, not one that takes a port in between.
The drivers also listen on the host's loopback without authentication while the run lasts (#101 items 8 and
9): run it only where no other account can reach them.

1. an encrypted vault; wallet "Fixture main" (`abandon … about`) and "Fixture second" (`legal winner … yellow`,
   BIP39 passphrase `TREZOR`), both from birth height 0;
2. dashd pays the main wallet 10 and 2.5 tDASH and the second 3 tDASH; the app pays a dashd address 1.25 tDASH
   (with a recipient label, which also files it in the address book); three blocks;
3. a transaction label, an address-book entry, a receive request with amount, label and message; username in
   requests on for the main wallet and unset for the second (v0.1.0: off for the main wallet); from v0.2.0 the main
   wallet's remaining received coin locked, and a watch-only wallet of BIP39's own vector `abandon … about` with passphrase `TREZOR` (account 0; the writer derives the xpub and first checks its derivation against the app's for the second wallet) that dashd
   pays 0.75 tDASH; dust protection 12345 duffs; backups kept 7; the saved network choice testnet; CoinJoin with
   multi-session, 3 sessions, 6 rounds, 25 DASH; theme dark, unit mDASH, 4 decimals, hidden balance, auto-lock
   900 s, subtract fee by default, masternodes shown;
4. the app quits, opens again offline (`--no-sync`), is unlocked, and its answers are recorded and checked against
   dashd (each balance equals the deposits less the payment and dashd's fee for it; from v0.2.0 every funded address
   of the two wallets with keys signs a message dashd verifies; a wallet's funded addresses sign only after each of
   that wallet's recorded coins matches dashd's unspent output in amount and address, a check the v0.2.0 fixture
   predates); it quits again and the data root is copied here.

A new release is covered by running the script for its tag, committing the directory, and adding the tag with
the SHA-256 of its `expected.json` (as `sha256sum` prints it) to `SNAPSHOT_SHA256` in the test. A committed
snapshot never changes: the test refuses one whose digest is not the pinned one, because the replay asks only
what is recorded, and a call or a value deleted from the snapshot is a check that no longer runs. The test needs
no other change unless the release renamed a field of a read command's answer (`RENAMED` / `DROPPED` in the
test, with the reason).

| Tag | Asset | SHA-256 | Written | dashd |
|---|---|---|---|---|
| v0.1.0 | `dash-wallet-0.1.0-linux-amd64.deb` | `1ac5c6690fc8394935fd62b7ff5b134d2206d8a856943553af264144f0b4b6a1` | 2026-11-19 | 24.0.0 |
| v0.2.0 | `dash-wallet-0.2.0-linux-amd64.deb` | `981af3a867cdd8560af5feb73c66cac443923d5541ed70c3440ff4a6b64f6e0f` | 2026-11-20 | 24.0.0 |
