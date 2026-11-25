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
- **Fork V4 / V5 dev files:** `{1,2,3,4}`, `{1,2,3,5}` and `{1,2,3,4,5}`.
  These stay refused until their adopting migrations and exact shapes are
  compiled in.
- **Hot WAL fork file:** a released-build file whose fork history row lives
  only in `wallet.sqlite-wal`, to replace the hand-built one in
  `refuses_a_wal_only_fork_v4_row_through_the_locked_check`.

## Done

- **Fork V3 with rows:** `data/fork-v3-dropped/wallet.sqlite`, a copy of
  the released v0.3.0 store into which the fork's own
  `drop_unauthorized_pending_spends` at c5917e6bd5 wrote three records
  (PROVENANCE.md). `tests::v3_dropped` runs it through the whole upgrade, a
  Strict open and a stored changeset with `core_dropped_unsigned_spends`
  byte-equal, its released coins unspent and loaded, and wallet removal
  deleting only that wallet's rows (Q93 T3 a–d). The table stays unadopted
  until P8. `data/fork-v3-no-accounts` (no account registrations) is kept
  only to show such a store is refused at the rehearsal.
