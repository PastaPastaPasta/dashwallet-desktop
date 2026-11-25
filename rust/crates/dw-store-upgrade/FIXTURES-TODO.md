# Fixtures still owed (Q78 R5)

The tests here run on copies of `testdata/upgrade/v0.1.0`, `v0.2.0` and
`v0.3.0`, which are never edited. These fixtures are still missing. Each one
is generated from fork pin c5917e6bd5 through the T4 dump crate, which reads
copies only and records the fork rev and each file's sha256.

- **Rotated contact:** a v0.3.0-shape `wallet.sqlite` whose
  `core_address_pool_retired` has real rows written by a contact rotation.
  It must be refused at a pin without P9a and must count correctly at the
  integration-3 pin (`rotated_contact_fork_file_counts_correctly_with_p9a`).
  Until it exists, the tests insert one retired row by hand
  (`tests::add_retired_row`).
- **Stamped contact:** a contact whose `external_account_reference` is
  non-NULL, to show R1's drop loses nothing a later build reads.
- **Fork V3 with rows:** `core_dropped_unsigned_spends` holding records,
  kept unadopted until P8 (fork V3 rule, pending its ruling).
- **Fork V4 / V5 dev files:** `{1,2,3,4}`, `{1,2,3,5}` and `{1,2,3,4,5}`.
  These stay refused until their adopting migrations and exact shapes are
  compiled in.
- **Hot WAL fork file:** a released-build file whose fork history row lives
  only in `wallet.sqlite-wal`, to replace the hand-built one in
  `refuses_a_wal_only_fork_v4_row_through_the_locked_check`.
