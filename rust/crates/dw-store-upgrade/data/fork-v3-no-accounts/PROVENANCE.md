# fork-v3-no-accounts

A fork V3 store whose `core_dropped_unsigned_spends` was written by the
fork's own `drop_unauthorized_pending_spends`, but whose wallets were
inserted with the fork's bare `wallet_db_in` test helper: it has no
`account_registrations`. The upstream Strict load refuses it ("required
account information is missing"), so the upgrade refuses it at the
rehearsal. Only the test that it is refused at the rehearsal with the tree
byte-equal
(`tests::v3_dropped::a_v3_store_the_upstream_load_cant_read_is_refused_at_the_rehearsal`)
may use it: its wallets are not real wallets, so it proves nothing about
records surviving the upgrade. The guard test
`tests::v3_dropped::the_no_accounts_store_serves_only_the_rehearsal_refusal`
fails if any other code in the crate names this directory. The fixture for
the upgrade itself is `fork-v3-dropped`.

- Fork rev: PastaPastaPasta/platform `c5917e6bd5a9ae8e4f3d6a51cbec08f7aeb68cb0`,
  crate `packages/rs-platform-wallet-storage`, toolchain 1.98.1.
- Generator: `generator.rs` (wallets inserted with the fork tests'
  `wallet_db_in` and one INSERT of the same shape), added to that tree as
  `src/sqlite/schema/core_state/tests/unauthorized_pending_spends/v3fixture_gen.rs`
  with `mod v3fixture_gen;` inside `mod unauthorized_pending_spends`.
- Command, from the platform workspace root:

  ```
  V3FIXTURE_OUT=<out>/wallet.sqlite cargo test -p platform-wallet-storage --lib -- \
    --ignored --exact --nocapture \
    sqlite::schema::core_state::tests::unauthorized_pending_spends::v3fixture_gen::generate_v3_dropped_unsigned_spends_fixture
  ```

- sha256:
  - `wallet.sqlite` c89e5d5a29477ab2ff075f0efd8c97c05d199462a2dc96b67185b2f21948101b
  - `generator.rs` 4f690029851fd500fdbd6fb9a09bd4162c28fdd95e00f4580af869aafab7ade2
  - `README.txt` 013ec965e3e0cf8911e44f989ee977e42a9d64c902a4d1e70318ef7d51303db5: the generation run's own record, with its
    sandbox id and local paths replaced (sha256 as written:
    0a0ef330994a01bfef50d65431dae4288aad001b6e430dd0228b907ef16033f5)
