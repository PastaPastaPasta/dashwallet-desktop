# fork-v3-dropped

`wallet.sqlite` is the released v0.3.0 store
(`testdata/upgrade/v0.3.0/data/regtest/wallet.sqlite`, sha256
1ae77e693d6ad446d7725e2e39011516b253580e42ac8f290fe8dba87b7d3504, real
wallets with their account registrations) after the fork's own drop path
wrote `core_dropped_unsigned_spends` records into a copy of it, for Q93
T3 (a) and (d). Tests read copies only (`tests::v3_dropped`).

- Fork rev: PastaPastaPasta/platform `c5917e6bd5a9ae8e4f3d6a51cbec08f7aeb68cb0`
  (`git archive` of that commit), crate `packages/rs-platform-wallet-storage`,
  default features, rustc 1.98.1, protoc 29.3.
- Generator: `generator.rs` in this directory (sha256
  1408bf61ec66151907bef85c07a61d08902a175e1ad8bacc19aaceaa4f3e4b3d), added to
  that tree as
  `src/sqlite/schema/core_state/tests/unauthorized_pending_spends/v3fixture_gen.rs`
  with one line in `src/sqlite/schema/core_state.rs`, the only other change:

  ```
  @@ -1823,6 +1823,7 @@
       mod unauthorized_pending_spends {
           use super::*;
  +        mod v3fixture_gen;
  ```

- Command, from the platform workspace root, with `<src>` a copy of the
  v0.3.0 `wallet.sqlite` and no log beside it:

  ```
  V3FIXTURE_SRC=<src>/wallet.sqlite V3FIXTURE_OUT=<out>/wallet.sqlite \
    cargo test -p platform-wallet-storage --lib -- --ignored --exact --nocapture \
    sqlite::schema::core_state::tests::unauthorized_pending_spends::v3fixture_gen::generate_v3_dropped_unsigned_spends_fixture
  ```

  Result: `test result: ok. 1 passed; 0 failed`, printing
  `A dropped 2 (locked 1c18ee68…95a0, pending 3f855563…abb1), B dropped 1
  (locked 7e81e941…3e06)` and no `-wal` or `-shm` after the persister closed.
- What the generator does, all through the fork's code: brings the copy to
  the fork's schema (`migrations::run`, already at fork V3); for wallets
  A (`0FF35F24…2ADD`) and B (`DA8B3E81…CA30`) stores, through
  `core_state::apply`, a coin paying one of the wallet's own unused BIP44
  receive scripts and an unsigned spend of it with change (A: one
  InstantSend-locked, one mempool; B: one InstantSend-locked, the lock rows
  written by `apply`); takes the store back to fork V1 with the fork tests'
  `as_fork_v1_left_it`; then opens it with `SqlitePersister::open`, which
  re-applies fork V2 and V3 and runs `drop_unauthorized_pending_spends`. It
  then checks the three records, their lock blobs and released coins, and
  that the Strict load returns each released coin unspent.
- sha256 of `wallet.sqlite`:
  d132793137d4fd554245fbd709bf13986225f7f2289b8a99914f1700d335fec8
  (778240 bytes, no `-wal` or `-shm`; header journal mode WAL).
