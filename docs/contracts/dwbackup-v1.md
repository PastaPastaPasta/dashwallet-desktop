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

`{"bundles": [<bundle>, …]}`, one bundle per wallet (§2). The container stays version 1; bundles
are version 2 (§2), and version 1 bundles are still read (§5).

## 2. Bundle (`WalletBackupBundle`, dw-vault)

```json
{"version":2, "vault_id":"<hex>", "network":"regtest", "wallet_id":"<hex>",
 "slot": {...}, "vault_wrapped_key": {"nonce":"<hex>","ct":"<hex>"},
 "records": {"mnemonic": {...}, "mnemonic_passphrase": {...}, "seed": {...}},
 "payload_salt":"<hex>", "payload": {"nonce":"<hex>","ct":"<hex>"}}
```

Every bundle has a **backup key** of its own: 32 random bytes drawn when the bundle is written.
It seals the records and the payload; the slot wraps only it. Nothing in a bundle unwraps the
source vault's data key (DEK) (review H2): a backup's passphrase opens that backup only — not
`vault.dwv`, not the vault's other wallets, not wallets added later, and a later passphrase change
or `encrypt` is not undone by an old backup.

The DEK is not rotated on `encrypt` or `change_passphrase`: slot B (quick unlock) wraps it under a
key only the OS biometric store holds, automatic `vault_key` bundles and version 1 bundles are
opened through it, and Dash Core's `walletpassphrasechange` keeps its master key too. With
per-backup keys a backup no longer extends the DEK's exposure; an old copy of `vault.dwv` itself
still opens with the passphrase it was written under, as any copy of an encrypted file does.

### 2.1 Records

The wallet's record payloads, each XChaCha20-Poly1305 under the backup key with AAD
`dw-vault/backup-record/v2 ‖ vault_id ‖ network ‖ wallet_id ‖ kind` (each field length-prefixed).
`seed` is always present; `mnemonic` and `mnemonic_passphrase` are absent for wallets imported from
a raw seed (`SeedDerivation::RawSeed`). The seed record carries the derivation (BIP39, Dash Core
quirks, raw seed).

### 2.2 Slot

How the backup key is recovered:

| `kind` | When | Unwrap |
|---|---|---|
| `vault_passphrase` | the source vault is encrypted | `kdf` and `salt` are the vault's slot P parameters at backup time: Argon2id(passphrase, salt, kdf) → KEK; `SHA-256("dw-vault/backup-kek/v2" ‖ KEK)` opens `wrapped_key` with AAD `dw-vault/backup-vault-slot/v2 ‖ vault_id ‖ network ‖ kdf ‖ salt`. The vault passphrase at backup time opens it. The writer holds that hash in memory while the vault is unlocked (derived whenever the passphrase is checked or set) and never stores it; without it the backup is refused as `backup.vault_locked`. |
| `backup_passphrase` | unencrypted vault, user backup | a new Argon2id slot (the vault's KDF policy, ≥ 256 MiB, t ≥ 3, ≥ 0.5 s) over the backup passphrase; the KEK opens `wrapped_key` with AAD `dw-vault/backup-slot/v2 ‖ vault_id ‖ network ‖ kdf ‖ salt`. |
| `vault_key` | unencrypted vault, automatic backup | none: only the source vault (same `vault_id`, DEK available) opens it, through `vault_wrapped_key`. An unencrypted vault has no passphrase to wrap with. |

`vault_wrapped_key` is the backup key sealed under `SHA-256("dw-vault/backup-vault-wrap/v2" ‖ DEK)`
with AAD `dw-vault/backup-vault-wrap/v2 ‖ vault_id ‖ network ‖ wallet_id`: the vault that wrote a
bundle opens it with its own DEK while that is available (unlocked or unencrypted), whatever the
slot, also after `encrypt` and `change_passphrase`. Passphrase attempts on a bundle are not
throttled: the file is the user's, not the vault's.

A reader refuses passphrase-slot KDF parameters above m = 4 GiB, t = 64 or p = 16 as
`backup.corrupt` (§5).

### 2.3 Payload

`payload_key = SHA-256("dw-vault/backup-payload/v1" ‖ backup key ‖ payload_salt)` (the key is
uniformly random, so a hash is a sufficient KDF); XChaCha20-Poly1305 with AAD
`dw-vault/backup-payload/v1 ‖ wallet_id ‖ vault_id ‖ header line`, each field length-prefixed.
The payload authenticates the whole file: records, slot and header are bound to it through the
key and the AAD.

Payload plaintext (JSON):

| Field | Meaning |
|---|---|
| `name` | Display name, or `null`. |
| `birth_height` | The wallet's birth height; a restore scans from it (0 when `null`). |
| `created_at` | When the wallet was added on the source device. |
| `app_rows` | The wallet's `app.sqlite` rows: `[{table, columns, rows}]`, every table with a `wallet_id` column plus `settings_kv` rows scoped to the wallet; the DashPay `dp_*` rows of the wallet are among them (§3.1). Values are `null` or `{"i": int}`, `{"r": real}`, `{"t": text}`, `{"b": base64}`. Rowid aliases are left out. |

The payload holds this wallet's data only and is built in memory (no temporary file). Backups
written before review M4 also carry `wallet_sqlite`, a base64 online backup of the whole network's
`wallet.sqlite` (every wallet's xpubs and history); it was never read back, readers ignore it, and
it is no longer written.

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
   book, receive requests, UTXO locks, wallet settings and the wallet's DashPay `dp_*` rows return
   (§3.1).

Wallet state is rebuilt by the compact-filter scan from the birth height. A restore that fails
after a wallet was registered (its app rows, or a later bundle of the file) removes the wallets it
registered, so a failed restore leaves the vault and the wallet list as they were.

### 3.1 DashPay rows (`dp_*`)

The wallet-scoped DashPay tables of `app.sqlite` (`DASHPAY.md` §3.4: `dp_main_identity`,
`dp_registration`, `dp_contest_watch`, `dp_events`, `dp_payment_lock`, `dp_trust_unverified`,
`dp_prefs`) have a `wallet_id` column, so they travel in `app_rows` with no change to this format.
Rowid aliases (`dp_registration.id`, `dp_events.id`) are renumbered on import, in export order, so event
order and read state survive. `dp_avatar` (the network-wide thumbnail cache index) has no wallet and is
not exported. Restoring an unverified-entity flag (`dp_trust_unverified`) is the conservative choice.

**Manager decision: `dp_registration` rows ARE restored on another machine**, so a registration whose
asset lock is already funded resumes there and the locked funds are not stranded. The wallet scan
rebuilds the asset-lock records, and the signing key is rederived from the seed, not read from the row.
A row can be behind reality, because automatic backups (§4) are not written on a registration
transition. The registration engine (DP1-02) therefore holds these conditions (`DASHPAY.md` §3.4,
"Registration rows"):

1. no funding of a restored row until SPV has synced and identities have been rediscovered;
2. an existing identity or recovered asset lock is adopted before anything is funded;
3. the outpoint is written when the lock is built, before it is broadcast;
4. an automatic backup runs when the flow reaches `FundingSent`.

An **invitation-funded** row does not resume on another machine: its link is a vault record
(`invitation/<id>`) that a bundle does not carry (§2.1), so the row fails there with a typed error
(`invitation.invalid`) and does not block a new registration. No funds of the user are involved, because
the asset lock is the inviter's.

## 4. Automatic backups (QT-116)

- File: `<network dir>/backups/<wallet id>.YYYY-MM-DD-HH-MM.dwbackup` (UTC), mode 0600.
- Written when a wallet is added (create, import, restore, keys attached) and for every wallet
  when a session opens, if the policy keeps any and the DEK is available (vault unlocked or
  unencrypted); otherwise skipped silently, as dash-qt backs up only what it can. A DashPay
  registration that reaches `FundingSent` also triggers one (§3.1), so the newest backup carries its
  asset-lock outpoint.
- Encrypted vault: slot `vault_passphrase`; unencrypted: `vault_key`. A user backup and an
  automatic backup of the same wallet can run at the same time: neither writes anything but its
  own destination file (created with `O_EXCL`, mode 0600).
- The newest `keep` per wallet are kept (`backup_policy`, 0..=10, default 10, dw-appdb setting
  `backup.keep`); lowering `keep` deletes the oldest at once.
- A failure sends `Notice{BackupFailed}` with the wallet id and cause.
- Watch-only wallets have no vault records and are not backed up (`backup_wallet` returns
  `NotImplemented{call: "backup_wallet.watch_only"}`).

## 5. Version 1 bundles and limits

Bundles with `"version":1` (written by the M2 build before review H2) are still read: their
records are the vault records as stored (under the source DEK, AAD
`dw-vault/record/v1 ‖ vault_id ‖ network ‖ schema_ver ‖ record_id`), the slot wraps the DEK
itself (`wrapped_dek`; `vault_passphrase` is a copy of slot P, `backup_passphrase` uses AAD
`dw-vault/backup-slot/v1 ‖ …`), there is no `vault_wrapped_key`, and the payload key is derived from
the DEK. They are never written again. Such a file hands out its vault's DEK to whoever knows its
passphrase; delete old ones once a new backup exists. The fixture
`testdata/dwbackup/v1_bundles_wrapped_dek.json` holds such bundles as written (field `wrapped_dek`).

A bundle `version` other than 1 or 2 is refused with `backup.unsupported_version{version}`. The
reader checks it on the raw JSON before parsing the bundle, so a newer bundle whose fields changed
is still reported as a newer version, not as corrupt; a bundle without a `version` is
`backup.corrupt`. (The M2 build from before review H2 reads version 1 only and reports a version 2
bundle as `backup.corrupt`.)

File-supplied Argon2id parameters above m = 4 GiB (4 194 304 KiB), t = 64 or p = 16 are refused
as corrupt before any memory is reserved: the production floor and calibration stay far below
them (256 MiB, t ≤ 24, p = 1), and a crafted file must not make the reader allocate or spin.
