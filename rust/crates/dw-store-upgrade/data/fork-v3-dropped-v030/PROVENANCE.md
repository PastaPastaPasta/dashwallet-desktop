# fork-v3-dropped-v030

v0.3.0's form: the same store and method as `data/fork-v3-dropped`, at the
platform pin v0.3.0 and its dev builds up to v0.3.0-dev.1117 use,
373342cb3. Its drop writes a kept row's absent spender in
`deleted_outputs` and `released_coins` as `""` (`hex(NULL)`); the nightly
builds' pin writes JSON null (`data/fork-v3-dropped`). Every other byte the
drop path writes is the same code at both pins (the diff of the fork
migrations, `run_for_open`, the persister open and the drop is that one
column expression). A P8 reader adopting these rows must accept both forms
for `spent_in_txid`.

`wallet.sqlite` is the released v0.3.0 store
(`testdata/upgrade/v0.3.0/data/regtest/wallet.sqlite`, sha256
1ae77e693d6ad446d7725e2e39011516b253580e42ac8f290fe8dba87b7d3504) after the
fork's own drop path wrote `core_dropped_unsigned_spends` records into a
copy of it. Tests read copies only (`tests::v3_dropped`).

- Fork rev: PastaPastaPasta/platform `373342cb380c407e0f1d910b8575e8c3c218680c`
  (`git archive` of that commit), crate `packages/rs-platform-wallet-storage`,
  default features, its own Cargo.lock (rusqlite 0.38, refinery 0.9.2),
  rustc 1.98.1, protoc 29.3.
- Generator: `data/fork-v3-dropped/generator.rs`, unchanged (sha256
  1408bf61ec66151907bef85c07a61d08902a175e1ad8bacc19aaceaa4f3e4b3d); it builds
  at this rev with no port. It is added to that tree as
  `src/sqlite/schema/core_state/tests/unauthorized_pending_spends/v3fixture_gen.rs`
  with one line in `src/sqlite/schema/core_state.rs`, the only other change:

  ```
  @@ -1822,6 +1822,7 @@
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
- What the generator does is as in `data/fork-v3-dropped/PROVENANCE.md`:
  claims seeded through `core_state::apply`, the store taken back to fork V1
  with the fork tests' `as_fork_v1_left_it`, then the real open that
  re-applies fork V2 and V3 and runs `drop_unauthorized_pending_spends`. No
  SQL of its own writes. Wallet B's owner-signed control claim is left out
  for the reason given there.
- sha256 of `wallet.sqlite`:
  0ab260fdfb032e2731c27e23aeb2fc174ef7f40f53beb179a4d56485bc356c5c
  (778240 bytes, no `-wal` or `-shm`; header journal mode WAL).
