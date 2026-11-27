# A data root written by desktop main f057bee77

`tests/main_written_store_opens.rs` opens a copy of `data/` with this build and requires the answers in
`main-observed.json` (`reopened_offline`), and a clean load of its `wallet.sqlite`.

## What it is

- `data/`: a data root, byte for byte as the engine of desktop main `f057bee778e486a88b7a10f7337d0d89637e8e40` left
  it after its last clean close and shutdown, less `instance.lock`. That build pins platform
  `PastaPastaPasta/platform@9d61fee3cc` (a descendant of `c5917e6bd5`) and rust-dashcore `a0f687ec62` (bip39 2.2.2).
  Nothing in it was written by any other build.
- `main-observed.json`: what that engine showed. `live` is the view with SPV running, before the close;
  `reopened_offline` is the view after it closed the network and opened it again with no SPV peer, the view this
  build must equal. `rescan_hold_marks_*` are the wallet's `rescan_hold:` rows in `app.sqlite` (`settings_kv`).
- `generator.rs`: the test that wrote both, a dw-engine integration test run at f057bee77. It is not built here.

Fixture data only: the wallet is BIP39's public `abandon … about` vector with no BIP39 passphrase, in an unencrypted
vault, on a private regtest chain that existed for the length of the run. No address in it holds anything anywhere
else.

## What the run did

Against dashd 24.0.0 on regtest (`regtest/scripts/fetch-dashcore.sh`), with a `miner` wallet and 101 blocks:

1. imported the wallet (birth height 0) and took four receive addresses; dashd paid them 10, 5, 2.5 and 1 DASH,
   one block each, then one more block; SPV synced to height 106;
2. locked the 2.5 DASH coin with coin control (`lock_outpoints`);
3. paid 0.5 DASH from the 5 DASH coin to dashd and pushed it into dashd's mempool, never mined: a pending own spend
   whose change (4.49999774 DASH) the engine withholds as `PendingUnproven` after a reopen;
4. built a 0.5 DASH payment from the 1 DASH coin that dashd refused (`prioritisetransaction` −10 DASH), turned
   dashd's network off, and abandoned it: the 1 DASH coin came back under a rescan hold whose rescan cannot finish
   (`rescan_hold:…:0` = `104,106`), so it is neither listed nor counted;
5. stopped SPV, closed the network, opened it again offline, recorded `reopened_offline`, closed it and shut the
   engine down.

To write it again (the txids and outpoints will differ):

```sh
git worktree add --detach <dir> f057bee778e486a88b7a10f7337d0d89637e8e40
cp generator.rs <dir>/rust/crates/dw-engine/tests/q5951_store_writer_main.rs
regtest/scripts/fetch-dashcore.sh <dashcore>
cd <dir>/rust && DASHD_PATH=<dashcore>/bin/dashd Q5951_OUT=<empty 0700 dir> \
  cargo test --locked -p dw-engine --test q5951_store_writer_main -- --nocapture
```

The run ends by asserting that `live` equals `reopened_offline`. At f057bee77 it does not: live, main counted the
pending spend's change in `confirmed`; after the reopen it withholds it. The files are written before that assert.
