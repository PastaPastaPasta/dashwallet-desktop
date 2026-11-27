# C3 legacy re-check fixture at c5917e6bd

- Fork pin: `c5917e6bd5a9ae8e4f3d6a51cbec08f7aeb68cb0` (v0.3.0-dev.1387).
- Generator: `generator.rs`, sha256 `8388f219e98c6dcfe3d750b83d1b0402e8b59cda9dce841f425eea25756b20e8`.
- Source store: the released v0.3.0 regtest store, `testdata/upgrade/v0.3.0/data/regtest/wallet.sqlite` in dashwallet-desktop, sha256 `1ae77e693d6ad446d7725e2e39011516b253580e42ac8f290fe8dba87b7d3504`.
- `wallet.sqlite`: sha256 `016cd7eabfcad5b0b2dcbbe24f3a32b2220c42fd0a1f8ae4e33137a8e6d3838a`, 786432 bytes, journal_mode wal, no `-wal`/`-shm` beside it.
- `wallet-islock.sqlite`: sha256 `a11b26ae7b1482f6ee892975451c103d97827b3521b8db9842b26b3c183b0491`, fixture (g), a separate store; see "(g) Lock-row fixture" at the end.
- `wallet-corrupt-blob.sqlite`: sha256 `a590380f3b3755b2dc82e847304bfe4fceda9ca8bc8386e8f0307ce5504ce5cd`, 786432 bytes. **Hand-modified by design**; see "Derived copy" below.
- Build: boat.dev large sandbox, rustc 1.98.1, libprotoc 29.3, `CARGO_INCREMENTAL=0`. Script `tools/gen.sh` (sha256 `34522ff67614e59d142aea229412b2fec66da7a012ba119c6b171739810d0a58`); log `logs/gen-c5917e6bd.log` (sha256 `af09e5338c2a1770f5abd842fc9f584b78f77837b043290e8c2fed1cbaf4b389`).

## How it was written

The pin's tree, from `git archive c5917e6bd`, with one change: `generator.rs` added as
`packages/rs-platform-wallet-storage/src/sqlite/schema/core_state/tests/unauthorized_pending_spends/c3fixture_gen.rs`, and one line in `core_state.rs`:

```
@@ -1823,6 +1823,7 @@
     mod unauthorized_pending_spends {
         use super::*;
+        mod c3fixture_gen;
```

Command, from the tree root:

```
C3FIXTURE_SRC=<copy of the v0.3.0 wallet.sqlite> C3FIXTURE_OUT=<out>/wallet.sqlite \
cargo test -p platform-wallet-storage --lib -- --ignored --exact --nocapture \
  sqlite::schema::core_state::tests::unauthorized_pending_spends::c3fixture_gen::generate_c3_legacy_recheck_fixture
```

Result: `test result: ok. 1 passed`, exit 0.

What the generator does, all through the pin's own code:

1. Runs the pin's migrations on a copy of the v0.3.0 store.
2. For each claimed coin, stores a funding transaction mined at height 50 through `core_state::apply`. The funding txid is the coin's outpoint txid. The coin pays one of the wallet's own unused BIP44 account 0 receive scripts.
3. Stores each claim as a Mempool record with the coin in `spent_utxos`, through `apply`. Each claim has one input and is signed by a key that isn't the coin's (sig by `owner(0x99)`, key of `owner(0x31)`). The generator asserts `verify_input_authorization == Ok(false)` for each, and asserts push-only-ness from the script's instructions.
4. Runs `as_fork_v1_left_it`, then a real non-Recovery `SqlitePersister::open`. That re-applies fork V2 and V3 and runs `drop_unauthorized_pending_spends`, which writes all 4 `core_dropped_unsigned_spends` rows. No row is inserted by hand.
5. Stores `spend_d`, mined at height 95 and spending coin (d), through `apply`, then opens the store with the persister a second time. The second open drops nothing more (the count stays at 4).
6. Copies the store out after a clean close.

## Rows by kind

Wallet A = `0FF35F248C69653EFCE8336CE14B78E29CD3BBAF4162C42246259850AF2C2ADD`, wallet B = `DA8B3E81EDB50926C70D3CD3C5DE4102FA7C1419B0559FE19D06813C64DCCA30`, the two wallets of the v0.3.0 store. Txids are shown in display order. The DB stores the internal order, byte-reversed; the `txid_internal` columns below are what `hex(txid)` returns.

| kind | dropped row id | wallet_id | claim txid (display) | input | claimed coin (outpoint blob) | why this kind |
|---|---|---|---|---|---|---|
| (a) | 1 | A | `f22699f7d1c7a3fa119a6c734e64fc0643c18da9d326d0fbc2254d3dfe8e99fc` | 0 | `2001A5C50F8EB6A91DD97BC167F3381BAE9D5916FCFB2E595570547D2A66B6CE9100` | script_sig `<sig> <key> OP_NOP` is not push-only; no push is the coin's key hash, so `verify_input_authorization` = `Ok(false)` (asserted). |
| (b) | 2 | A | `b9ee9599b9e3fe2d4e7bf7997fbbdb1e7f52edecfaab9adba98e7043c66eabdd` | 0 | `204185550E700E883D8B7FF2625778CAB104C4027029F8F35B7CE65136E17BC5B300` | push-only `<sig> <key>`, wrong key, `Ok(false)`. It pays B's receive script. B holds the same txid mined at height 60 (InBlock, stored through `apply`) with its receipt coin `20DDAB6EC643708EA9DB9AABFAECED527F1EDBBB7F99F77B4E2DFEE3B99995EEB900` (490000). The fork's skip check reads block_info and locks only in the same wallet, so it dropped the claim in A. |
| (c) | 4 | B | `00958be2e62ef41a80a6d56cc4c8295e979cc491df36b758a49e0c08b8535374` | 0 | `20A2502EA8AB1BB94B4E32328B58FA7A9B302F14FF9731B2217AEE111BB761CBC700` | push-only, wrong key, `Ok(false)`. Its txid is mined or locked nowhere, it has no descendant, and its coin is not spent by anything else. |
| (d) | 3 | A | `2779809a779ef0ac770de30cea61bb66bcd561fea07953aaa9d9f48efffb94bc` | 0 | `20ABC4162ACB3F98BFCD00B4032599420A3B7DD37EF7D42B752DAB9617B45741A900` | push-only, wrong key, `Ok(false)`. After the drop freed the coin, `8623fd8588d1e357a7eba0122668dccb0cd6a78f043ffa4cc6fdc7363b95ec0c` (mined at height 95, stored through `apply` at this pin) spends it. `core_utxos` has the coin as spent=1. |

Funding transactions (display txid; all mined at height 50, in the claiming wallet):
- (a) `91ceb6662a7d547055592efbfc16599dae1b38f367c17bd91da9b68e0fc5a501`
- (b) `b3c57be13651e67c5bf3f8297002c404b1ca785762f27f8b3d880e700e558541`
- (c) `c7cb61b71b11ee7a21b23197ff142f309b7afa588b32324e4bb91baba82e50a2`
- (d) `a94157b41796ab2d752bd4f77ed37d3b0a42992503b400cdbf983fcb2a16c4ab`

In `released_coins`, `spent_in_txid` is `null` at this pin.

No lock row was needed for (b): it is settled in wallet B by being mined there. The fork writes no `core_instant_locks` row in this fixture (the count is 0), and every dropped row has `islock_blob` NULL.

## sqlite3 reads

All reads below were run on a scratch copy (`cp`, then `cmp` against the fixture before and after). The fixture was never opened in place. Script: `tools/reads.sh` (sha256 `2428d5a132f59362900c91f28aae30ae3ef757a6b2482c71fc1a988934aa5ee8`), sqlite3 3.50.6.

```

$ sqlite3 -readonly -header -column <copy> "PRAGMA journal_mode;"
journal_mode
------------
wal         

$ sqlite3 -readonly -header -column <copy> "SELECT id, hex(wallet_id) AS wallet_id, hex(txid) AS txid_internal, length(record_blob) AS blob_len, length(islock_blob) AS islock_len, islock_verified, deleted_outputs, released_coins FROM core_dropped_unsigned_spends ORDER BY id;"
id  wallet_id                                                         txid_internal                                                     blob_len  islock_len  islock_verified  deleted_outputs  released_coins                                                                                                                                                                                                                                       
--  ----------------------------------------------------------------  ----------------------------------------------------------------  --------  ----------  ---------------  ---------------  -----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
1   0FF35F248C69653EFCE8336CE14B78E29CD3BBAF4162C42246259850AF2C2ADD  FC998EFE3D4D25C2FBD026D3A98DC14306FC644E736C9A11FAA3C7D1F79926F2  228                                    []               [{"outpoint":"2001A5C50F8EB6A91DD97BC167F3381BAE9D5916FCFB2E595570547D2A66B6CE9100","value":500000,"script":"76A9143A2D4145A4F098523B3E8127F1DA87CFC55B8E7988AC","spent":1,"spent_in_txid":null,"winner_mined_height":null,"is_sweep_placeholder":0}]
2   0FF35F248C69653EFCE8336CE14B78E29CD3BBAF4162C42246259850AF2C2ADD  DDAB6EC643708EA9DB9AABFAECED527F1EDBBB7F99F77B4E2DFEE3B99995EEB9  228                                    []               [{"outpoint":"204185550E700E883D8B7FF2625778CAB104C4027029F8F35B7CE65136E17BC5B300","value":500000,"script":"76A914D3C0870E8E13A9EC320F1280889127AA15A4C0A188AC","spent":1,"spent_in_txid":null,"winner_mined_height":null,"is_sweep_placeholder":0}]
3   0FF35F248C69653EFCE8336CE14B78E29CD3BBAF4162C42246259850AF2C2ADD  BC94FBFF8EF4D9A9AA5379A0FE61D5BC66BB61EA0CE30D77ACF09E779A807927  228                                    []               [{"outpoint":"20ABC4162ACB3F98BFCD00B4032599420A3B7DD37EF7D42B752DAB9617B45741A900","value":500000,"script":"76A9144C2E2BF28069045A8EB627C839FF2306B73BBA3088AC","spent":1,"spent_in_txid":null,"winner_mined_height":null,"is_sweep_placeholder":0}]
4   DA8B3E81EDB50926C70D3CD3C5DE4102FA7C1419B0559FE19D06813C64DCCA30  745353B8080C9EA458B736DF91C49C975E29C8C46CD5A6801AF42EE6E28B9500  227                                    []               [{"outpoint":"20A2502EA8AB1BB94B4E32328B58FA7A9B302F14FF9731B2217AEE111BB761CBC700","value":500000,"script":"76A914B7DFE7152F0A00457A5F8688DB92ACF721A9C78088AC","spent":1,"spent_in_txid":null,"winner_mined_height":null,"is_sweep_placeholder":0}]

$ sqlite3 -readonly -header -column <copy> "SELECT hex(wallet_id) AS wallet_id, hex(outpoint) AS outpoint, value, spent, spent_in_txid IS NULL AS spent_in_is_null, hex(spent_in_txid) AS spent_in_txid FROM core_utxos WHERE hex(outpoint) IN ('2001A5C50F8EB6A91DD97BC167F3381BAE9D5916FCFB2E595570547D2A66B6CE9100','204185550E700E883D8B7FF2625778CAB104C4027029F8F35B7CE65136E17BC5B300','20A2502EA8AB1BB94B4E32328B58FA7A9B302F14FF9731B2217AEE111BB761CBC700','20ABC4162ACB3F98BFCD00B4032599420A3B7DD37EF7D42B752DAB9617B45741A900','20DDAB6EC643708EA9DB9AABFAECED527F1EDBBB7F99F77B4E2DFEE3B99995EEB900') ORDER BY wallet_id, outpoint;"
wallet_id                                                         outpoint                                                              value   spent  spent_in_is_null  spent_in_txid
----------------------------------------------------------------  --------------------------------------------------------------------  ------  -----  ----------------  -------------
0FF35F248C69653EFCE8336CE14B78E29CD3BBAF4162C42246259850AF2C2ADD  2001A5C50F8EB6A91DD97BC167F3381BAE9D5916FCFB2E595570547D2A66B6CE9100  500000  0      1                              
0FF35F248C69653EFCE8336CE14B78E29CD3BBAF4162C42246259850AF2C2ADD  204185550E700E883D8B7FF2625778CAB104C4027029F8F35B7CE65136E17BC5B300  500000  0      1                              
0FF35F248C69653EFCE8336CE14B78E29CD3BBAF4162C42246259850AF2C2ADD  20ABC4162ACB3F98BFCD00B4032599420A3B7DD37EF7D42B752DAB9617B45741A900  500000  1      1                              
DA8B3E81EDB50926C70D3CD3C5DE4102FA7C1419B0559FE19D06813C64DCCA30  20A2502EA8AB1BB94B4E32328B58FA7A9B302F14FF9731B2217AEE111BB761CBC700  500000  0      1                              
DA8B3E81EDB50926C70D3CD3C5DE4102FA7C1419B0559FE19D06813C64DCCA30  20DDAB6EC643708EA9DB9AABFAECED527F1EDBBB7F99F77B4E2DFEE3B99995EEB900  490000  0      1                              

$ sqlite3 -readonly -header -column <copy> "SELECT hex(wallet_id) AS wallet_id, hex(txid) AS txid_internal, height, finalized, length(record_blob) AS blob_len FROM core_transactions WHERE hex(txid) IN ('FC998EFE3D4D25C2FBD026D3A98DC14306FC644E736C9A11FAA3C7D1F79926F2','DDAB6EC643708EA9DB9AABFAECED527F1EDBBB7F99F77B4E2DFEE3B99995EEB9','745353B8080C9EA458B736DF91C49C975E29C8C46CD5A6801AF42EE6E28B9500','BC94FBFF8EF4D9A9AA5379A0FE61D5BC66BB61EA0CE30D77ACF09E779A807927','0CEC953B36C7FDC64CFA3F048FA7D60CCBDC682612A0EBA757E3D18885FD2386','01A5C50F8EB6A91DD97BC167F3381BAE9D5916FCFB2E595570547D2A66B6CE91','4185550E700E883D8B7FF2625778CAB104C4027029F8F35B7CE65136E17BC5B3','A2502EA8AB1BB94B4E32328B58FA7A9B302F14FF9731B2217AEE111BB761CBC7','ABC4162ACB3F98BFCD00B4032599420A3B7DD37EF7D42B752DAB9617B45741A9') ORDER BY wallet_id, height;"
wallet_id                                                         txid_internal                                                     height  finalized  blob_len
----------------------------------------------------------------  ----------------------------------------------------------------  ------  ---------  --------
0FF35F248C69653EFCE8336CE14B78E29CD3BBAF4162C42246259850AF2C2ADD  01A5C50F8EB6A91DD97BC167F3381BAE9D5916FCFB2E595570547D2A66B6CE91  50      1          161     
0FF35F248C69653EFCE8336CE14B78E29CD3BBAF4162C42246259850AF2C2ADD  4185550E700E883D8B7FF2625778CAB104C4027029F8F35B7CE65136E17BC5B3  50      1          161     
0FF35F248C69653EFCE8336CE14B78E29CD3BBAF4162C42246259850AF2C2ADD  ABC4162ACB3F98BFCD00B4032599420A3B7DD37EF7D42B752DAB9617B45741A9  50      1          161     
0FF35F248C69653EFCE8336CE14B78E29CD3BBAF4162C42246259850AF2C2ADD  0CEC953B36C7FDC64CFA3F048FA7D60CCBDC682612A0EBA757E3D18885FD2386  95      1          161     
DA8B3E81EDB50926C70D3CD3C5DE4102FA7C1419B0559FE19D06813C64DCCA30  A2502EA8AB1BB94B4E32328B58FA7A9B302F14FF9731B2217AEE111BB761CBC7  50      1          161     
DA8B3E81EDB50926C70D3CD3C5DE4102FA7C1419B0559FE19D06813C64DCCA30  DDAB6EC643708EA9DB9AABFAECED527F1EDBBB7F99F77B4E2DFEE3B99995EEB9  60      1          268     

$ sqlite3 -readonly -header -column <copy> "SELECT COUNT(*) AS instant_locks FROM core_instant_locks;"
instant_locks
-------------
0            

$ sqlite3 -readonly -header -column <copy> "SELECT id, hex(substr(record_blob,1,16)) AS blob_head16, length(record_blob) AS blob_len FROM core_dropped_unsigned_spends ORDER BY id;"
id  blob_head16                       blob_len
--  --------------------------------  --------
1   0300012001A5C50F8EB6A91DD97BC167  228     
2   030001204185550E700E883D8B7FF262  228     
3   03000120ABC4162ACB3F98BFCD00B403  228     
4   03000120A2502EA8AB1BB94B4E32328B  227
```

## Derived copy: wallet-corrupt-blob.sqlite (hand-modified by design)

This file is **not** written by the fork. The fork cannot write an unparseable `record_blob`: the column is `BLOB NOT NULL`, and the drop's INSERT is its only writer. That INSERT copies the blob of a `core_transactions` row that it selected `record_blob IS NOT NULL` and that decoded through `readable_record` with a matching txid. The modification exists only to exercise an undecodable legacy row.

- Row changed: `core_dropped_unsigned_spends` id **4**, kind **(c)** (wallet B, claim `00958be2…5374`).
- Why (c): its untouched verdict is DROP, so a re-hold of this row can only come from the undecodable blob.
- Exact statement, run with sqlite3 3.50.6 on a copy of `wallet.sqlite`:

```
UPDATE core_dropped_unsigned_spends SET record_blob = unhex('FFFFFFFF' || substr(hex(record_blob), 9)) WHERE id = 4 AND hex(txid) = '745353B8080C9EA458B736DF91C49C975E29C8C46CD5A6801AF42EE6E28B9500';
```

- The first 4 bytes, `03000120`, became `FFFFFFFF`. Same length (227), still `typeof = blob`, non-NULL, tail unchanged.
- Script and output: `tools/corrupt.sh` (sha256 `7cf56374620fbd4ac12e3cf9eb3c9e0ac6ae5fd8aefad16043561b8876e4155e`):

```
before: type=blob len=227 sha256=9c907743b8f9c1da9859258fb9f63abea26783a1378837dc0d41a5b7bac4fc4f
before head8=03000120A2502EA8
UPDATE: UPDATE core_dropped_unsigned_spends SET record_blob = unhex('FFFFFFFF' || substr(hex(record_blob), 9)) WHERE id = 4 AND hex(txid) = '745353B8080C9EA458B736DF91C49C975E29C8C46CD5A6801AF42EE6E28B9500';
changes=1
after: type=blob len=227 sha256=117eb790f51bdf0f772bdc35685fb93b9436ec9bba94378c7e58ad5144831426
after head8=FFFFFFFFA2502EA8
after tail equals before tail: yes
== file sizes
/work/scratch/pb-migration/fixtures-c3/rd/work-c5917e6bd/before.sqlite 786432
/work/scratch/pb-migration/fixtures-c3/rd/work-c5917e6bd/after.sqlite 786432
== differing file bytes (cmp -l: offset(1-based) before(octal) after(octal))
366489   3 377
366490   0 377
366491   1 377
366492  40 377
== logical dump diff (excluding nothing)
1288c1288
< INSERT INTO core_dropped_unsigned_spends VALUES(4,X'da8b3e81edb50926c70d3cd3c5de4102fa7c1419b0559fe19d06813c64dcca30',X'745353b8080c9ea458
---
> INSERT INTO core_dropped_unsigned_spends VALUES(4,X'da8b3e81edb50926c70d3cd3c5de4102fa7c1419b0559fe19d06813c64dcca30',X'745353b8080c9ea458
016cd7eabfcad5b0b2dcbbe24f3a32b2220c42fd0a1f8ae4e33137a8e6d3838a  c5917e6bd/wallet.sqlite
a590380f3b3755b2dc82e847304bfe4fceda9ca8bc8386e8f0307ce5504ce5cd  c5917e6bd/wallet-corrupt-blob.sqlite
```

`cmp -l` shows the 4 blob bytes are the only bytes of the file that differ. The SQLite header, the other pages and the other rows are byte-identical. The `.dump` diff is the single `INSERT` line for row 4 (cut at 140 columns above).

Why it no longer decodes: the record is bincode-serde with `config::standard()` (little-endian, varint). Its first field is the transaction version (`u16`, varint), and `0xFF` is a reserved varint discriminant, so decoding fails at byte 0.

This was confirmed with the pin's own `blob::decode::<TransactionRecord>` and `readable_record`, through an extra ignored test, `tools/c3fixture_check.rs` (sha256 `923df49225f9ebe227fb6565e617e6803fa2f1cf266610ac41ee7dd212d828ec`). It is a scratch tree change (`mod c3fixture_check;`) only and is not in the fixture. Log `logs/check-c5917e6bd.log` (sha256 `936ec6354d298c8e2435b6672b2cf3ad8ec48876ec4892e5a3b1f8045d0d25cf`):

```
== wallet
C3CHECK id=1 type=blob len=228 readable_record=true decode=ok txid=f22699f7d1c7a3fa119a6c734e64fc0643c18da9d326d0fbc2254d3dfe8e99fc
C3CHECK id=2 type=blob len=228 readable_record=true decode=ok txid=b9ee9599b9e3fe2d4e7bf7997fbbdb1e7f52edecfaab9adba98e7043c66eabdd
C3CHECK id=3 type=blob len=228 readable_record=true decode=ok txid=2779809a779ef0ac770de30cea61bb66bcd561fea07953aaa9d9f48efffb94bc
C3CHECK id=4 type=blob len=227 readable_record=true decode=ok txid=00958be2e62ef41a80a6d56cc4c8295e979cc491df36b758a49e0c08b8535374
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 506 filtered out; finished in 0.00s
== exit 0
== wallet-corrupt-blob
C3CHECK id=1 type=blob len=228 readable_record=true decode=ok txid=f22699f7d1c7a3fa119a6c734e64fc0643c18da9d326d0fbc2254d3dfe8e99fc
C3CHECK id=2 type=blob len=228 readable_record=true decode=ok txid=b9ee9599b9e3fe2d4e7bf7997fbbdb1e7f52edecfaab9adba98e7043c66eabdd
C3CHECK id=3 type=blob len=228 readable_record=true decode=ok txid=2779809a779ef0ac770de30cea61bb66bcd561fea07953aaa9d9f48efffb94bc
C3CHECK id=4 type=blob len=227 readable_record=false decode=err bincode decode error
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 506 filtered out; finished in 0.00s
== exit 0
```

Before the UPDATE, all four rows decode and pass `readable_record`. After it, only row 4 fails.

## Notes on faithfulness

- **Unsigned by construction.** The claims are built with a wrong key, not taken from real broadcasts. The generator has no wallet keys, which matches how the fork's drop sees an unauthorized spend.
- **No change outputs.** Unlike `data/fork-v3-dropped/generator.rs`, each claim has a single output paying away (or paying B for (b)). So `deleted_outputs` is `[]` on every row.
- **`dropped_at`** comes from the open's `unixepoch()` at generation time and differs between runs. It is the only non-deterministic content; every printed txid and outpoint was identical at both pins.
- **(d) `spent_in_txid`.** At this pin, `apply` leaves `spent_in_txid` NULL on coin (d) after `spend_d` and sets `spent` = 1. The later spend is proven by the mined `core_transactions` row `0CEC…2386` (height 95), whose record spends the coin.
- **L5.** At this pin, `record_blob` is `BLOB NOT NULL` and is written only from a blob that decoded. So an undecodable legacy row cannot come from the fork's own writes; only the hand-modified copy above has one.

## (g) Lock-row fixture: wallet-islock.sqlite

This is a separate store, so `wallet.sqlite` and `wallet-corrupt-blob.sqlite` and the (a)–(d) row ids are unchanged. It holds one dropped row, (g), built through the pin.

- `wallet-islock.sqlite`: sha256 `a11b26ae7b1482f6ee892975451c103d97827b3521b8db9842b26b3c183b0491`, 778240 bytes, journal_mode wal, no `-wal`/`-shm` beside it.
- Generator: `generator-islock.rs`, sha256 `e3a3ff6bd3904a8c031de53c09db356b3f8af75fa8056604aefeec895bc4da6d`.
- Source: the same v0.3.0 store (`1ae77e69…7d3504`).
- Build: a fresh boat, rustc 1.98.1, libprotoc 29.3. Script `tools/gen-islock.sh` (sha256 `9245d0f356110124d55c851f8dbb3b340b3fa22aa333708d5af09156d34e28e7`); log `logs/gen-islock-c5917e6bd.log` (sha256 `e9e5bf02cb433bedd37c96d8b5d9eb1d59b853b9dc0b5cee89ec1163cf56b5af`); `test result: ok. 1 passed`, exit 0.
- Tree change: `generator-islock.rs` added as `.../core_state/tests/unauthorized_pending_spends/c3islock_gen.rs`, and one line in `core_state.rs`:

```
@@ -1823,6 +1823,7 @@
     mod unauthorized_pending_spends {
         use super::*;
+        mod c3islock_gen;
```

Command:

```
C3FIXTURE_SRC=<copy of the v0.3.0 wallet.sqlite> C3FIXTURE_OUT=<out>/wallet-islock.sqlite \
cargo test -p platform-wallet-storage --lib -- --ignored --exact --nocapture \
  sqlite::schema::core_state::tests::unauthorized_pending_spends::c3islock_gen::generate_c3_islock_fixture
```

Printed: `C3FIXTURE kind=g id=1 wallet=0FF35F248C69653EFCE8336CE14B78E29CD3BBAF4162C42246259850AF2C2ADD claim=651d12af56e37477d019b9c29136560ebdb82ca137c98cf01ae4667c784f2178 input=0 coin=20C2106C7530C4E46B2CB4C32DA2D450EE225DE28AA674137548BABBEC7A47B11F00 funding=1fb1477aecbbba48751374a68ae25d22ee50d4a22dc3b42c6be4c430756c10c2 islock_verified=0 islock_len=165`

How the lock row got there, all through the pin's code:

1. In wallet A, `apply` stores a funding transaction mined at height 50 (`1fb1477a…10c2`). It pays A's 4th unused BIP44 account 0 receive script.
2. It stores a push-only claim of that coin, signed by the wrong key (`verify_input_authorization` = `Ok(false)`, asserted). The claim is recorded as `TransactionContext::InstantSend(lock)` with the coin in `spent_utxos`, and its lock is in `instant_locks_for_non_final_records`. The pin's apply writes the `core_instant_locks` row with `verified = 1` (core_state.rs:183-186). The generator asserts that row is present, and that the claim's `height` IS NULL.
3. `as_fork_v1_left_it` drops the `verified` column, as a fork V1 build had none. The lock row stays; this is asserted.
4. A real non-Recovery `SqlitePersister::open` runs fork V2 (`migrations_fork/V2__verified_instant_locks.rs`: `ADD COLUMN verified INTEGER NOT NULL DEFAULT 0`). The lock row therefore reads `verified = 0`. `verified_instant_lock` (called at :535, the drop skip check; its query reads only `verified = 1` rows) finds nothing, so the drop does not skip the claim.
5. The drop's INSERT (core_state.rs:616) copies `l.islock_blob` and `l.verified` into the dropped row, and then it deletes the `core_instant_locks` row (core_state.rs:630).

Asserted after the open: exactly one dropped row; `islock_blob` == `blob::encode(&lock)` of the stored lock; `islock_verified` = 0; no `core_instant_locks` row for the txid; no `core_transactions` row for the txid in any wallet; the coin is unspent again in A; B unchanged.

| kind | row id | wallet_id | claim txid (display) | input | claimed coin | islock_verified | islock_blob sha256 |
|---|---|---|---|---|---|---|---|
| (g) | 1 | A `0FF35F24…2ADD` | `651d12af56e37477d019b9c29136560ebdb82ca137c98cf01ae4667c784f2178` | 0 | `20C2106C7530C4E46B2CB4C32DA2D450EE225DE28AA674137548BABBEC7A47B11F00` | 0 | `2b23d13d68f23f158b2d1bbe34c397e739c514b791e960938ba5692502559c63` (165 bytes) |

Notes:
- **Not a real lock.** The lock is `InstantLock { txid, ..InstantLock::default() }`: version 1, no inputs, zero cycle hash and zero BLS signature, the same shape as the pin's own `locked` test helper. It is not a quorum-signed lock, and it names the claim's txid.
- **Released coins.** In `released_coins`, `spent_in_txid` is `null` at this pin.
- **No change output.** The claim pays one output away, so `deleted_outputs` is `[]`.
- **Lock blob identical at both pins.** The `islock_blob` sha256 is the same at both pins.

### sqlite3 reads (scratch copy; the fixture was compared with `cmp` and never opened in place)

Script `tools/reads-islock.sh` (sha256 `7fa890503d700c112f70411fb65ce0667cb2a6e94a0abbe9801318b6f2bbf0f9`):

```

$ sqlite3 -readonly -header -column <copy> "PRAGMA journal_mode;"
journal_mode
------------
wal         

$ sqlite3 -readonly -header -column <copy> "SELECT id, hex(wallet_id) AS wallet_id, hex(txid) AS txid_internal, length(record_blob) AS blob_len, length(islock_blob) AS islock_len, islock_verified, deleted_outputs, released_coins FROM core_dropped_unsigned_spends ORDER BY id;"
id  wallet_id                                                         txid_internal                                                     blob_len  islock_len  islock_verified  deleted_outputs  released_coins                                                                                                                                                                                                                                       
--  ----------------------------------------------------------------  ----------------------------------------------------------------  --------  ----------  ---------------  ---------------  -----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
1   0FF35F248C69653EFCE8336CE14B78E29CD3BBAF4162C42246259850AF2C2ADD  78214F787C66E41AF08CC937A12CB8BD0E563691C2B919D07774E356AF121D65  392       165         0                []               [{"outpoint":"20C2106C7530C4E46B2CB4C32DA2D450EE225DE28AA674137548BABBEC7A47B11F00","value":500000,"script":"76A914628D3DDD3A9413983A0F5AD854FB1C0F8791CD5288AC","spent":1,"spent_in_txid":null,"winner_mined_height":null,"is_sweep_placeholder":0}]

$ sqlite3 -readonly -header -column <copy> "SELECT id, hex(islock_blob) AS islock_blob FROM core_dropped_unsigned_spends ORDER BY id;"
id  islock_blob                                                                                                                                                                                                                                                                                                                               
--  ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
1   01002078214F787C66E41AF08CC937A12CB8BD0E563691C2B919D07774E356AF121D6520000000000000000000000000000000000000000000000000000000000000000060000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000

$ sqlite3 -readonly -header -column <copy> "SELECT COUNT(*) AS lock_rows_for_txid FROM core_instant_locks WHERE hex(txid) = '78214F787C66E41AF08CC937A12CB8BD0E563691C2B919D07774E356AF121D65';"
lock_rows_for_txid
------------------
0                 

$ sqlite3 -readonly -header -column <copy> "SELECT COUNT(*) AS lock_rows_total FROM core_instant_locks;"
lock_rows_total
---------------
0              

$ sqlite3 -readonly -header -column <copy> "SELECT COUNT(*) AS tx_rows_for_txid_any_wallet FROM core_transactions WHERE hex(txid) = '78214F787C66E41AF08CC937A12CB8BD0E563691C2B919D07774E356AF121D65';"
tx_rows_for_txid_any_wallet
---------------------------
0                          

$ sqlite3 -readonly -header -column <copy> "SELECT hex(wallet_id) AS wallet_id, hex(txid) AS txid_internal, height, finalized FROM core_transactions WHERE hex(txid) = 'C2106C7530C4E46B2CB4C32DA2D450EE225DE28AA674137548BABBEC7A47B11F';"
wallet_id                                                         txid_internal                                                     height  finalized
----------------------------------------------------------------  ----------------------------------------------------------------  ------  ---------
0FF35F248C69653EFCE8336CE14B78E29CD3BBAF4162C42246259850AF2C2ADD  C2106C7530C4E46B2CB4C32DA2D450EE225DE28AA674137548BABBEC7A47B11F  50      1        

$ sqlite3 -readonly -header -column <copy> "SELECT hex(wallet_id) AS wallet_id, hex(outpoint) AS outpoint, value, spent, spent_in_txid IS NULL AS spent_in_is_null FROM core_utxos WHERE hex(outpoint) = '20C2106C7530C4E46B2CB4C32DA2D450EE225DE28AA674137548BABBEC7A47B11F00';"
wallet_id                                                         outpoint                                                              value   spent  spent_in_is_null
----------------------------------------------------------------  --------------------------------------------------------------------  ------  -----  ----------------
0FF35F248C69653EFCE8336CE14B78E29CD3BBAF4162C42246259850AF2C2ADD  20C2106C7530C4E46B2CB4C32DA2D450EE225DE28AA674137548BABBEC7A47B11F00  500000  0      1
```
