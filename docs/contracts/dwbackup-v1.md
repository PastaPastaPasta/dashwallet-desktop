# `.dwbackup` format, version 1

Status: **contract**, 2026-10-06 (R2, M2). Writer and reader: `rust/crates/dw-engine/src/backup.rs`
(container) and `rust/crates/dw-vault/src/vault/compat.rs` (key material). Calls:
[`m2-engine.md`](m2-engine.md) §2.7 (`backup_wallet`, `restore_backup`, `automatic_backups`,
`backup_policy`, `set_backup_policy`).

A `.dwbackup` holds one wallet of one network: what is needed to rebuild it on another machine
or in another vault, protected so that the file alone reveals nothing.

## 1. Container

Three lines, UTF-8, `\n` line ends:

```text
DWBACKUP 1\n
<header JSON>\n
<body JSON>\n
```

The first line is the magic `DWBACKUP ` and the decimal format version. A reader refuses other
versions with `backup.unsupported_version{version}`.

### 1.1 Header (plain, authenticated)

```json
{"format":"dwbackup","format_version":1,"network":"regtest","created_at":1791263000,
 "wallet_ids":["<64 hex>"],"automatic":false,"app_version":"0.1.0"}
```

| Field | Meaning |
|---|---|
| `network` | The network directory name: `mainnet`, `testnet`, `regtest`, `devnet-<name>`. Restoring into another network fails with `backup.network_mismatch`. |
| `created_at` | UNIX seconds. |
| `wallet_ids` | One id per bundle, in body order. v1 writers write one. |
| `automatic` | Written by the rotation (QT-116), not by the user (QT-110). |

`inspect_wallet_file` reads only this line (`WalletFileKind::DwBackup`). The exact bytes of the
header line are the AAD of every payload (§2.3), so editing the header breaks the backup.

### 1.2 Body

`{"bundles": [<bundle>, …]}`, one bundle per wallet (§2).

## 2. Bundle (`WalletBackupBundle`, dw-vault)

```json
{"version":1, "vault_id":"<hex>", "network":"regtest", "wallet_id":"<hex>",
 "slot": {...}, "records": {"mnemonic": {...}, "mnemonic_passphrase": {...}, "seed": {...}},
 "payload_salt":"<hex>", "payload": {"nonce":"<hex>","ct":"<hex>"}}
```

### 2.1 Records

The wallet's vault records exactly as the source vault stores them: XChaCha20-Poly1305 under the
vault's data key (DEK), AAD = `dw-vault/record/v1 ‖ vault_id ‖ network ‖ schema_ver ‖ record_id`
(dw-vault `file.rs`). `seed` is always present; `mnemonic` and `mnemonic_passphrase` are absent for
wallets imported from a raw seed (`SeedDerivation::RawSeed`). The seed record carries the
derivation (BIP39, Dash Core quirks, raw seed).

### 2.2 Slot

How the DEK of the source vault is recovered:

| `kind` | When | Unwrap |
|---|---|---|
| `vault_passphrase` | the source vault is encrypted | a copy of the vault's passphrase slot: Argon2id(passphrase, salt, kdf) → KEK; AAD as slot P (`dw-vault/slot-p/v1 ‖ vault_id ‖ network ‖ kdf ‖ salt`). The vault passphrase at backup time opens it. |
| `backup_passphrase` | unencrypted vault, user backup | a new Argon2id slot (the vault's KDF policy, ≥ 256 MiB, t ≥ 3, ≥ 0.5 s) over the backup passphrase; AAD `dw-vault/backup-slot/v1 ‖ vault_id ‖ network ‖ kdf ‖ salt`. |
| `vault_key` | unencrypted vault, automatic backup | none: only the source vault (same `vault_id`, DEK available) opens it. An unencrypted vault has no passphrase to wrap with, and storing the DEK in the file would make it plaintext. |

The vault that wrote a bundle opens it with its own DEK while that is available (unlocked or
unencrypted), whatever the slot. Passphrase attempts on a bundle are not throttled: the file is
the user's, not the vault's.

### 2.3 Payload

`payload_key = SHA-256("dw-vault/backup-payload/v1" ‖ DEK ‖ payload_salt)` (the DEK is uniformly
random, so a hash is a sufficient KDF); XChaCha20-Poly1305 with AAD
`dw-vault/backup-payload/v1 ‖ wallet_id ‖ vault_id ‖ header line`, each field length-prefixed.
The payload authenticates the whole file: records, slot and header are bound to it through the
key and the AAD.

Payload plaintext (JSON):

| Field | Meaning |
|---|---|
| `name` | Display name, or `null`. |
| `birth_height` | The wallet's birth height; a restore scans from it (0 when `null`). |
| `created_at` | When the wallet was added on the source device. |
| `app_rows` | The wallet's `app.sqlite` rows: `[{table, columns, rows}]`, every table with a `wallet_id` column plus `settings_kv` rows scoped to the wallet. Values are `null` or `{"i": int}`, `{"r": real}`, `{"t": text}`, `{"b": base64}`. Rowid aliases are left out. |
| `wallet_sqlite` | Base64 of a SQLite online backup of `wallet.sqlite` (SqlitePersister `backup_to`). |

## 3. Restore

`restore_backup(path, passphrase)`:

1. Read the file (≤ 1 GiB), check magic, version and network.
2. Open every bundle (DEK, records, payload) before anything is stored. Wrong passphrase:
   `backup.wrong_passphrase`; a passphrase slot and no passphrase: `backup.passphrase_required`;
   any authentication failure: `backup.corrupt`.
3. Check that the seed derives `wallet_id` on this network.
4. Store the secret under this vault's DEK and register the wallet (seed-safety order, as
   `import_wallet`), with the payload's name and birth height. A wallet registered with keys
   already: `backup.already_exists`.
5. Insert the `app_rows` (`INSERT OR IGNORE`; identical rows are skipped), so labels, address
   book, receive requests, UTXO locks and wallet settings return.

Wallet state is rebuilt by the compact-filter scan from the birth height. **v1 does not read
`wallet_sqlite` back**: replacing the live `wallet.sqlite` would drop the other wallets of the
network, and merging one wallet's rows is a platform-wallet-storage feature that does not exist
yet. The snapshot is carried so a later version or offline tooling can use it.

## 4. Automatic backups (QT-116)

- File: `<network dir>/backups/<wallet id>.YYYY-MM-DD-HH-MM.dwbackup` (UTC), mode 0600.
- Written when a wallet is added (create, import, restore, keys attached) and for every wallet
  when a session opens, if the policy keeps any and the DEK is available (vault unlocked or
  unencrypted); otherwise skipped silently, as dash-qt backs up only what it can.
- Encrypted vault: slot `vault_passphrase`; unencrypted: `vault_key`.
- The newest `keep` per wallet are kept (`backup_policy`, 0..=10, default 10, dw-appdb setting
  `backup.keep`); lowering `keep` deletes the oldest at once.
- A failure sends `Notice{BackupFailed}` with the wallet id and cause.
- Watch-only wallets have no vault records and are not backed up (`backup_wallet` returns
  `NotImplemented{call: "backup_wallet.watch_only"}`).
