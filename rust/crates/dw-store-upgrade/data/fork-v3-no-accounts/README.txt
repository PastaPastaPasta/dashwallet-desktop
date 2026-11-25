V3 dropped-unsigned-spends fixture (wallet.sqlite)
==================================================

Made by: the PastaPastaPasta/platform fork at commit
  c5917e6bd5a9ae8e4f3d6a51cbec08f7aeb68cb0
  ("feat(platform-wallet): fund platform addresses from a shielded lock's remaining value")
  crate: packages/rs-platform-wallet-storage (platform-wallet-storage), default features
  source copied from ~/.cargo/git/checkouts/platform-fff45202c37f3b1f/c5917e6 (no .git/target)
Built and run on boat.dev sandbox bx_99x4y6qb (type large, --no-env), toolchain 1.98.1
from the tree's rust-toolchain.toml, PROTOC = protoc 27.3 release binary
(Ubuntu's protoc 3.21.12 is rejected by tenderdash-proto). Generated 2026-11-25 ~02:46Z.

Test name (lib unit test, #[ignore]d, run explicitly):
  sqlite::schema::core_state::tests::unauthorized_pending_spends::v3fixture_gen::generate_v3_dropped_unsigned_spends_fixture

Command (from the platform workspace root):
  V3FIXTURE_OUT=/home/user/out/wallet.sqlite cargo test -p platform-wallet-storage --lib -- \
    --ignored --exact --nocapture \
    sqlite::schema::core_state::tests::unauthorized_pending_spends::v3fixture_gen::generate_v3_dropped_unsigned_spends_fixture
  -> "test result: ok. 1 passed; 0 failed" (see generate.log, attempt 3)

Only change to the fork's source: generator.rs added as
  src/sqlite/schema/core_state/tests/unauthorized_pending_spends/v3fixture_gen.rs
and this one-line hook in src/sqlite/schema/core_state.rs (it is a child module, so it uses
the fork's own private test helpers via `use super::*`):
--- /work/scratch/dwrepin/m3/v3fixture/work/core_state.rs.orig	2026-11-25 02:40:31.221839805 +0000
+++ /work/scratch/dwrepin/m3/v3fixture/work/platform/packages/rs-platform-wallet-storage/src/sqlite/schema/core_state.rs	2026-11-25 02:46:11.399815345 +0000
@@ -1823,6 +1823,7 @@
 
     mod unauthorized_pending_spends {
         use super::*;
+        mod v3fixture_gen;
         use dashcore::secp256k1::Message;
         use dashcore::sighash::SighashCache;
         use dashcore::TxIn;

How the generator works (all via the fork's own code and test helpers)
-----------------------------------------------------------------------
1. A file DB gets the fork's full schema via `wallet_db_in` (crate::sqlite::migrations::run,
   upstream V001..V018 + fork V1..V3). It inserts wallet A (A1*32) and wallet B (B2*32), both
   'testnet', with the same raw INSERT the fork's `wallet_db_in` uses.
2. Each claim is seeded with the fork's `claim_in_store` helper, the way the fork tests
   do it. The helper stores a wallet coin through `core_state::apply`. It adds the BIP44 pool row
   for the coin's script (`own_pool_row`, the fork tests' raw INSERT). Then it applies a
   change set with the spend record (signed by key 0x99, not the owner 0x31), its 190,000
   change output to the wallet, and the coin marked spent. For the `locked` context the
   change set also carries `instant_locks_for_non_final_records`, so `apply` writes a
   `core_instant_locks` row (core_state.rs:181-195; the lock is InstantLock::default() with
   txid set, i.e. a test vector, all-zero signature).
     A: funding 0x61 unsigned claim, InstantSend-locked
     A: funding 0x62 unsigned claim, Mempool (no lock row)
     B: funding 0x64 unsigned claim, InstantSend-locked
     B: funding 0x65 owner-signed claim, Mempool (control: must survive)
3. The fork's `as_fork_v1_left_it` (core_state.rs:2106) takes the store back to how a fork V1
   build left it: it deletes dw_fork_schema_history rows >= 2, drops
   core_dropped_unsigned_spends, and drops core_instant_locks.verified. This is the fork's own
   test seeding (raw SQL), as in `a_v1_root_is_copied_then_repaired_on_its_first_open`.
4. `SqlitePersister::open(SqlitePersisterConfig::new(path))` (the fork's real open, persister.rs:
   ~384-413) takes the pre-migration auto backup into the tempdir. It then runs `run_for_open`,
   which re-applies fork V2 (adds `verified` DEFAULT 0, so the V1-era lock rows read as
   unverified) and fork V3 (creates core_dropped_unsigned_spends). Next it calls
   `drop_unauthorized_pending_spends` (core_state.rs:505) in an IMMEDIATE transaction. The
   unverified lock does not exempt a claim (core_state.rs:536-540 checks only verified=1
   locks), so the three unsigned claims are dropped. The INSERT ... LEFT JOIN
   core_instant_locks (core_state.rs:616-621) copies islock_blob/islock_verified, then deletes
   the tx row, its lock row and its change output, and sets the claimed coin unspent.
5. The persister is dropped. No -wal or -shm was left beside the DB ("V3FIXTURE after
   persister drop: -wal exists=false -shm exists=false"), so the clean close came from the
   persister closing its own connection. No separate PRAGMA wal_checkpoint was needed or run
   (that fallback branch in generator.rs did not execute). The file was then copied to
   V3FIXTURE_OUT before any other connection opened it. The test then asserted the post-drop
   state on the original (read-only open).

Note for consumers: the header journal_mode is WAL. Open a copy in a writable dir (or use
?immutable=1), since a WAL open creates -shm/-wal beside the file.

sha256(wallet.sqlite) = c89e5d5a29477ab2ff075f0efd8c97c05d199462a2dc96b67185b2f21948101b
size = 372736 bytes; no wallet.sqlite-wal / -shm present.

Generator stdout (from generate.log). The txids are in display order; the DB stores them in
internal byte order, i.e. reversed:
  V3FIXTURE ok: A dropped 2 (locked e17bef770e5e4cd33388f0b0e1b346f92f3371875d3e9625e6e30887825b6bc6, pending 8132926420f65506f987647dc206bb747859113b6e67c4e30d875e94f405166c), B dropped 1 (locked ddeedfc87fe18efdecb8643905e15bb30fd1c0a3c3ddb45d4c49adbb1a529447), B kept signed bbd00737c0a70f58ef7068e913440a0d0310e63c71448f58bba56ce6c0f77d25

Verification (sqlite3 3.50.6 on a COPY of wallet.sqlite)
---------------------------------------------------------
Queries:
.headers on
.mode list
SELECT '-- wallets';
SELECT hex(wallet_id) AS wallet_id, network, birth_height FROM wallets ORDER BY wallet_id;
SELECT '-- core_dropped_unsigned_spends';
SELECT id, hex(wallet_id) AS wallet_id, hex(txid) AS txid, length(record_blob) AS record_len, length(islock_blob) AS islock_len, islock_verified, dropped_at FROM core_dropped_unsigned_spends ORDER BY id;
SELECT '-- released_coins / deleted_outputs';
SELECT id, released_coins, deleted_outputs FROM core_dropped_unsigned_spends ORDER BY id;
SELECT '-- islock_blob hex';
SELECT id, hex(islock_blob) FROM core_dropped_unsigned_spends WHERE islock_blob IS NOT NULL ORDER BY id;
SELECT '-- released coins now in core_utxos';
SELECT d.id AS drop_id, hex(u.wallet_id) AS wallet_id, hex(u.outpoint) AS outpoint, u.value, u.spent, u.spent_in_txid, u.is_sweep_placeholder
  FROM core_dropped_unsigned_spends d, json_each(d.released_coins) j
  JOIN core_utxos u ON u.wallet_id = d.wallet_id AND hex(u.outpoint) = json_extract(j.value, '$.outpoint')
  ORDER BY d.id;
SELECT '-- all core_utxos';
SELECT hex(wallet_id) AS wallet_id, hex(outpoint) AS outpoint, value, spent, hex(spent_in_txid) AS spent_in_txid FROM core_utxos ORDER BY wallet_id, outpoint;
SELECT '-- remaining core_transactions / core_instant_locks';
SELECT hex(wallet_id) AS wallet_id, hex(txid) AS txid, height FROM core_transactions ORDER BY wallet_id;
SELECT COUNT(*) AS instant_lock_rows FROM core_instant_locks;
SELECT '-- refinery_schema_history';
SELECT MAX(version) AS max_version, COUNT(*) AS rows FROM refinery_schema_history;
SELECT version, name, checksum FROM refinery_schema_history ORDER BY version;
SELECT '-- dw_fork_schema_history';
SELECT version, name, applied_on, checksum FROM dw_fork_schema_history ORDER BY version;
SELECT '-- contacts.external_account_reference';
SELECT name, type FROM pragma_table_info('contacts') WHERE name = 'external_account_reference';
SELECT '-- core_instant_locks.verified';
SELECT name, type, dflt_value FROM pragma_table_info('core_instant_locks') WHERE name = 'verified';
SELECT '-- schema of core_dropped_unsigned_spends';
SELECT sql FROM sqlite_master WHERE tbl_name='core_dropped_unsigned_spends';
SELECT '-- pragmas';
PRAGMA journal_mode;
PRAGMA application_id;
PRAGMA integrity_check;

Output:
'-- wallets'
-- wallets
wallet_id|network|birth_height
A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1|testnet|0
B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2|testnet|0
'-- core_dropped_unsigned_spends'
-- core_dropped_unsigned_spends
id|wallet_id|txid|record_len|islock_len|islock_verified|dropped_at
1|A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1|C66B5B828708E3E625963E5D8771332FF946B3E1B0F08833D34C5E0E77EF7BE1|424|165|0|1795574802
2|A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1|6C1605F4945E870DE3C4676E3B11597874BB06C27D6487F90655F62064923281|258|||1795574802
3|B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2|4794521ABBAD494C5DB4DDC3A3C0D10FB35BE1053964B8ECFD8EE17FC8DFEEDD|424|165|0|1795574802
'-- released_coins / deleted_outputs'
-- released_coins / deleted_outputs
id|released_coins|deleted_outputs
1|[{"outpoint":"20616161616161616161616161616161616161616161616161616161616161616100","value":500000,"script":"76A9148320611FF032223C1F4BB1FBBD2291FD2B3F43D988AC","spent":1,"spent_in_txid":null,"winner_mined_height":null,"is_sweep_placeholder":0}]|[{"outpoint":"20C66B5B828708E3E625963E5D8771332FF946B3E1B0F08833D34C5E0E77EF7BE101","value":190000,"script":"76A9148320611FF032223C1F4BB1FBBD2291FD2B3F43D988AC","spent":0,"spent_in_txid":null,"winner_mined_height":null,"is_sweep_placeholder":0}]
2|[{"outpoint":"20626262626262626262626262626262626262626262626262626262626262626200","value":500000,"script":"76A9148320611FF032223C1F4BB1FBBD2291FD2B3F43D988AC","spent":1,"spent_in_txid":null,"winner_mined_height":null,"is_sweep_placeholder":0}]|[{"outpoint":"206C1605F4945E870DE3C4676E3B11597874BB06C27D6487F90655F6206492328101","value":190000,"script":"76A9148320611FF032223C1F4BB1FBBD2291FD2B3F43D988AC","spent":0,"spent_in_txid":null,"winner_mined_height":null,"is_sweep_placeholder":0}]
3|[{"outpoint":"20646464646464646464646464646464646464646464646464646464646464646400","value":500000,"script":"76A9148320611FF032223C1F4BB1FBBD2291FD2B3F43D988AC","spent":1,"spent_in_txid":null,"winner_mined_height":null,"is_sweep_placeholder":0}]|[{"outpoint":"204794521ABBAD494C5DB4DDC3A3C0D10FB35BE1053964B8ECFD8EE17FC8DFEEDD01","value":190000,"script":"76A9148320611FF032223C1F4BB1FBBD2291FD2B3F43D988AC","spent":0,"spent_in_txid":null,"winner_mined_height":null,"is_sweep_placeholder":0}]
'-- islock_blob hex'
-- islock_blob hex
id|hex(islock_blob)
1|010020C66B5B828708E3E625963E5D8771332FF946B3E1B0F08833D34C5E0E77EF7BE120000000000000000000000000000000000000000000000000000000000000000060000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000
3|0100204794521ABBAD494C5DB4DDC3A3C0D10FB35BE1053964B8ECFD8EE17FC8DFEEDD20000000000000000000000000000000000000000000000000000000000000000060000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000
'-- released coins now in core_utxos'
-- released coins now in core_utxos
drop_id|wallet_id|outpoint|value|spent|spent_in_txid|is_sweep_placeholder
1|A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1|20616161616161616161616161616161616161616161616161616161616161616100|500000|0||0
2|A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1|20626262626262626262626262626262626262626262626262626262626262626200|500000|0||0
3|B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2|20646464646464646464646464646464646464646464646464646464646464646400|500000|0||0
'-- all core_utxos'
-- all core_utxos
wallet_id|outpoint|value|spent|spent_in_txid
A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1|20616161616161616161616161616161616161616161616161616161616161616100|500000|0|
A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1|20626262626262626262626262626262626262626262626262626262626262626200|500000|0|
B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2|20257DF7C0E66CA5BB588F44713CE610030D0A4413E96870EF580FA7C03707D0BB01|190000|0|
B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2|20646464646464646464646464646464646464646464646464646464646464646400|500000|0|
B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2|20656565656565656565656565656565656565656565656565656565656565656500|500000|1|
'-- remaining core_transactions / core_instant_locks'
-- remaining core_transactions / core_instant_locks
wallet_id|txid|height
A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1|6161616161616161616161616161616161616161616161616161616161616161|50
A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1|6262626262626262626262626262626262626262626262626262626262626262|50
B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2|6464646464646464646464646464646464646464646464646464646464646464|50
B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2|6565656565656565656565656565656565656565656565656565656565656565|50
B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2|257DF7C0E66CA5BB588F44713CE610030D0A4413E96870EF580FA7C03707D0BB|
instant_lock_rows
0
'-- refinery_schema_history'
-- refinery_schema_history
max_version|rows
18|18
version|name|checksum
1|initial|16458714021387590417
2|address_height_pin|9304453063389616695
3|invitations|6768710306020099565
4|asset_lock_recovered_status|14446754812616312642
5|dpns_name_states|16834970739141546284
6|tracked_masternodes|13718514320935649081
7|utxo_sweep_winner_height|4410976868559573281
8|rehydration_base_schema|8883371305674850298
9|unified|8389295906542582065
10|pool_public_key|15088205568906581463
11|pool_reserved_at|1184102594786438566
12|drop_core_utxo_metadata|14281733277552006158
13|shielded_viewing_keys|2886147678304763509
14|single_source_core_confirmation_height|13462228277166821110
15|purge_legacy_empty_script_spent_utxos|723175730717787103
16|identity_keys_null_scope_requires_existing_identity|5573841766625796742
17|identity_scan_state|14086773559107496341
18|identity_hard_delete|14764815300602197850
'-- dw_fork_schema_history'
-- dw_fork_schema_history
version|name|applied_on|checksum
1|dashpay_external_rotation|2026-11-25T02:46:42.842446257Z|13037568497809321770
2|verified_instant_locks|2026-11-25T02:46:42.901337691Z|7947297170294142560
3|dropped_unsigned_spends_recovery_record|2026-11-25T02:46:42.901345592Z|14354827916965344210
'-- contacts.external_account_reference'
-- contacts.external_account_reference
name|type
external_account_reference|INTEGER
'-- core_instant_locks.verified'
-- core_instant_locks.verified
name|type|dflt_value
verified|INTEGER|0
'-- schema of core_dropped_unsigned_spends'
-- schema of core_dropped_unsigned_spends
sql
CREATE TABLE core_dropped_unsigned_spends (
    id INTEGER PRIMARY KEY,
    wallet_id BLOB NOT NULL,
    txid BLOB NOT NULL,
    record_blob BLOB NOT NULL,
    islock_blob BLOB NULL,
    islock_verified INTEGER NULL,
    deleted_outputs TEXT NOT NULL,
    released_coins TEXT NOT NULL,
    dropped_at INTEGER NOT NULL DEFAULT (unixepoch()),
    FOREIGN KEY (wallet_id) REFERENCES wallets(wallet_id) ON DELETE CASCADE
)
CREATE INDEX idx_core_dropped_unsigned_spends_txid
    ON core_dropped_unsigned_spends(wallet_id, txid)
'-- pragmas'
-- pragmas
journal_mode
wal
application_id
1347180372
integrity_check
ok
