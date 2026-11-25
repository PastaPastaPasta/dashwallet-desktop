//! What this build's platform-wallet-storage embeds, read at runtime from a
//! fresh store, and the behavioural P9a probe (Q78-A A1–A7).
//!
//! The embedded migration list is `pub(crate)` upstream (only `__test-helpers`
//! widens it, which downstream must not enable), so the build is asked by
//! running it: a fresh store in a private 0700 directory under the data root
//! (A6) records every embedded migration in `refinery_schema_history` and
//! builds the schema they create.
//!
//! P9a is decided by behaviour, not by schema or name: fe5b9da942 has the
//! retired table without the accounting and load fixes. The schema only tells
//! "no retired-table support" (Absent) from "supported"; the behaviour decides
//! Present against Broken.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use dashcore::{Address, OutPoint, ScriptBuf, Transaction, TxIn, TxOut, Txid};
use key_wallet::account::{AccountType, StandardAccountType};
use key_wallet::bip32::ExtendedPubKey;
use key_wallet::managed_account::ManagedCoreFundsAccount;
use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
use key_wallet::managed_account::transaction_record::{
    OutputDetail, OutputRole, TransactionDirection, TransactionRecord,
};
use key_wallet::transaction_checking::{TransactionContext, TransactionType};
use key_wallet::wallet::Wallet;
use key_wallet::wallet::initialization::WalletAccountCreationOptions;
use key_wallet::{Account, Network, Utxo};
use platform_wallet::changeset::{
    AccountAddressPoolEntry, AccountRegistrationEntry, ClientWalletStartState, CoreChangeSet,
    PlatformWalletChangeSet, PlatformWalletPersistence,
};
use platform_wallet_storage::{SqlitePersister, SqlitePersisterConfig};
use rusqlite::{Connection, params};

use crate::fork;
use crate::sql::{has_table, open_existing};

/// One embedded upstream migration as a fresh store records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedMigration {
    pub version: i64,
    pub name: String,
    pub checksum: String,
}

/// A `sqlite_master` row of a fresh store, statement as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FreshObject {
    pub kind: String,
    pub name: String,
    pub table: String,
    pub sql: Option<String>,
}

/// The P9a probe's outcome (Q78-A A4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum P9aStatus {
    /// Retired-table support with every check and control passing: fork files
    /// open, and P9a adopts the retired table.
    Present,
    /// No retired-table support, controls passing: a fork file whose
    /// `core_address_pool_retired` has rows is refused; an empty one opens.
    Absent,
    /// Supported but a check or control failed, or the probe failed: every
    /// wallet open is refused.
    Broken { detail: String },
}

/// What this build embeds, and its P9a status.
#[derive(Clone, Debug)]
pub struct Adopters {
    pub(crate) embedded: Vec<EmbeddedMigration>,
    pub(crate) fresh: Vec<FreshObject>,
    pub(crate) p9a: P9aStatus,
    pub(crate) observations: Vec<String>,
}

/// P4's migration name: adopts fork V2 (`core_instant_locks.verified`).
pub(crate) const P4_NAME: &str = "instant_lock_verified";

static EMBEDDED: OnceLock<Adopters> = OnceLock::new();

impl Adopters {
    /// This build's adopters, probed once per process in a private directory
    /// under `data_root` (the parent of every network directory), which is
    /// created 0700 first. Blocking: call it from `spawn_blocking`, before
    /// the first wallet open.
    pub fn for_build(data_root: &Path) -> &'static Adopters {
        EMBEDDED.get_or_init(|| {
            let adopters = Self::probe_new_root(data_root);
            tracing::info!(p9a = ?adopters.p9a, observations = ?adopters.observations,
                "store upgrade: platform-wallet-storage probe");
            adopters
        })
    }

    /// The probe, uncached, after creating `data_root` 0700 if missing.
    pub(crate) fn probe_new_root(data_root: &Path) -> Adopters {
        match create_private_dir_all(data_root) {
            Ok(()) => Self::probe(data_root),
            Err(e) => Self::broken(format!("create the data root {}: {e}", data_root.display())),
        }
    }

    /// The probe's result, if it has run in this process.
    pub fn cached() -> Option<&'static Adopters> {
        EMBEDDED.get()
    }

    #[cfg(test)]
    pub(crate) fn cache_for_test(probe: impl FnOnce() -> Adopters) -> &'static Adopters {
        EMBEDDED.get_or_init(probe)
    }

    fn broken(detail: String) -> Adopters {
        Adopters {
            embedded: Vec::new(),
            fresh: Vec::new(),
            p9a: P9aStatus::Broken {
                detail: format!("probe failed: {detail}"),
            },
            observations: Vec::new(),
        }
    }

    /// Run the probe now, uncached.
    pub(crate) fn probe(data_root: &Path) -> Adopters {
        let mut observations = Vec::new();
        let result = ProbeDir::create(data_root).and_then(|dir| {
            let (embedded, fresh) = read_fresh_store(dir.path())?;
            let supported = fresh
                .iter()
                .any(|o| o.kind == "table" && o.name == fork::RETIRED_POOL);
            observations.push(format!(
                "retired-table support in the fresh schema: {supported}"
            ));
            let behaviour = run_behaviour(dir.path(), &mut observations);
            Ok((embedded, fresh, decide(supported, behaviour)))
        });
        match result {
            Ok((embedded, fresh, p9a)) => Adopters {
                embedded,
                fresh,
                p9a,
                observations,
            },
            Err(detail) => Adopters {
                observations,
                ..Self::broken(detail)
            },
        }
    }

    /// The P9a probe's outcome.
    pub fn p9a_status(&self) -> &P9aStatus {
        &self.p9a
    }

    /// What the probe saw, for the log and the evidence packet.
    pub fn observations(&self) -> &[String] {
        &self.observations
    }

    /// The highest embedded migration version.
    pub fn max_version(&self) -> i64 {
        self.embedded.iter().map(|m| m.version).max().unwrap_or(0)
    }

    pub fn embedded(&self) -> &[EmbeddedMigration] {
        &self.embedded
    }

    pub(crate) fn has_migration(&self, name: &str) -> bool {
        self.embedded.iter().any(|m| m.name == name)
    }

    pub(crate) fn fresh_table_sql(&self, table: &str) -> Option<&str> {
        self.fresh
            .iter()
            .find(|o| o.kind == "table" && o.name == table)
            .and_then(|o| o.sql.as_deref())
    }

    #[cfg(test)]
    pub(crate) fn with_p9a(&self, p9a: P9aStatus) -> Adopters {
        Adopters {
            p9a,
            ..self.clone()
        }
    }
}

/// Whether this build carries a working P9a.
pub fn p9a_present(adopters: &Adopters) -> bool {
    adopters.p9a == P9aStatus::Present
}

/// Whether MG's R1d line ("you can't pay {contact} by username yet") must
/// ship: only a build whose P9a reads Present drops it (Q78-A R1 amendment §3).
pub fn r1d_line_required(adopters: &Adopters) -> bool {
    !p9a_present(adopters)
}

fn decide(supported: bool, behaviour: Result<Checks, String>) -> P9aStatus {
    let checks = match behaviour {
        Ok(checks) => checks,
        Err(detail) => return P9aStatus::Broken { detail },
    };
    match (
        supported,
        checks.controls_failed.is_empty(),
        checks.retired_failed.is_empty(),
    ) {
        (_, false, _) => P9aStatus::Broken {
            detail: format!("controls failed: {}", checks.controls_failed.join("; ")),
        },
        (false, true, _) => P9aStatus::Absent,
        (true, true, true) => P9aStatus::Present,
        (true, true, false) => P9aStatus::Broken {
            detail: format!(
                "retired checks failed: {}",
                checks.retired_failed.join("; ")
            ),
        },
    }
}

/// Name prefix of the probe's private directory under the data root.
pub(crate) const PROBE_PREFIX: &str = ".store-upgrade-probe-";

/// A 0700 directory under the data root, removed on drop: it lives only for
/// the probe's call, so one is left behind only by a crash
/// ([`crate::sweep_stale_scratch`]).
struct ProbeDir(tempfile::TempDir);

impl ProbeDir {
    fn create(data_root: &Path) -> Result<Self, String> {
        let dir = tempfile::Builder::new()
            .prefix(PROBE_PREFIX)
            .tempdir_in(data_root)
            .map_err(|e| format!("create probe dir under {}: {e}", data_root.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
                .map_err(|e| format!("chmod probe dir: {e}"))?;
        }
        let backups = dir.path().join("backups").join("auto");
        create_private_dir_all(&backups).map_err(|e| format!("probe backup dir: {e}"))?;
        Ok(Self(dir))
    }

    fn path(&self) -> &Path {
        self.0.path()
    }
}

fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

fn persister_config(dir: &Path, file: &Path) -> SqlitePersisterConfig {
    SqlitePersisterConfig::new(file).with_auto_backup_dir(Some(dir.join("backups").join("auto")))
}

fn open_persister(dir: &Path, file: &Path) -> Result<SqlitePersister, String> {
    SqlitePersister::open(persister_config(dir, file))
        .map_err(|e| format!("open {}: {e}", file.display()))
}

fn read_fresh_store(dir: &Path) -> Result<(Vec<EmbeddedMigration>, Vec<FreshObject>), String> {
    let path = dir.join("fresh.sqlite");
    drop(open_persister(dir, &path)?);
    let conn = open_existing(&path).map_err(|e| format!("reopen fresh store: {e}"))?;
    let embedded = conn
        .prepare("SELECT version, name, checksum FROM refinery_schema_history ORDER BY version")
        .and_then(|mut s| {
            s.query_map([], |r| {
                Ok(EmbeddedMigration {
                    version: r.get(0)?,
                    name: r.get(1)?,
                    checksum: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|e| format!("read embedded history: {e}"))?;
    let fresh = conn
        .prepare("SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name")
        .and_then(|mut s| {
            s.query_map([], |r| {
                Ok(FreshObject {
                    kind: r.get(0)?,
                    name: r.get(1)?,
                    table: r.get(2)?,
                    sql: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|e| format!("read fresh schema: {e}"))?;
    if embedded.is_empty() {
        return Err("the fresh store recorded no migrations".into());
    }
    Ok((embedded, fresh))
}

/// The probe's fork-shaped file: the released v0.1.0 fixture (fork V1,
/// refinery V018) with its viewing key and second wallet removed
/// (`scripts/make-probe-template.py`).
pub(crate) const PROBE_TEMPLATE: &[u8] = include_bytes!("../data/probe-fork-v1.sqlite");
#[cfg(test)]
pub(crate) const PROBE_TEMPLATE_SHA256: &str =
    "c3409e74a4849c2f438e497b6dcacbaece4a7cd1a9adf3a483c18bb594d4c413";

const OWNER: [u8; 32] = [0x71; 32];
const FRIEND: [u8; 32] = [0x72; 32];
const V_S1: u64 = 7_100_001;
const V_S2: u64 = 7_200_003;
const V_LIVE: u64 = 7_300_005;
const V_CONTACT: u64 = 7_400_007;

#[derive(Default)]
struct Checks {
    controls_failed: Vec<String>,
    retired_failed: Vec<String>,
}

fn bip44() -> AccountType {
    AccountType::Standard {
        index: 0,
        standard_account_type: StandardAccountType::BIP44Account,
    }
}

fn contact_account() -> AccountType {
    AccountType::DashpayExternalAccount {
        index: 0,
        user_identity_id: OWNER,
        friend_identity_id: FRIEND,
    }
}

fn contact_xpub(seed: u8) -> Result<ExtendedPubKey, String> {
    Wallet::from_seed_bytes(
        [seed; 64],
        Network::Testnet,
        WalletAccountCreationOptions::Default,
    )
    .map_err(|e| format!("contact wallet: {e}"))?
    .get_bip44_account(0)
    .map(|a| a.account_xpub)
    .ok_or_else(|| "contact wallet has no BIP44 account 0".to_string())
}

fn contact_pools(
    xpub: ExtendedPubKey,
    network: Network,
) -> Result<Vec<AccountAddressPoolEntry>, String> {
    let account = Account {
        parent_wallet_id: None,
        account_type: contact_account(),
        network,
        account_xpub: xpub,
        is_watch_only: true,
    };
    let managed = ManagedCoreFundsAccount::from_account(&account)
        .map_err(|e| format!("contact pool: {e}"))?;
    let entries: Vec<AccountAddressPoolEntry> = managed
        .managed_account_type()
        .address_pools()
        .into_iter()
        .filter(|pool| !pool.addresses().is_empty())
        .map(|pool| AccountAddressPoolEntry {
            account_type: contact_account(),
            pool_type: pool.pool_type(),
            addresses: pool.addresses().values().cloned().collect(),
        })
        .collect();
    if entries.is_empty() {
        return Err("the contact account derives no pool".into());
    }
    Ok(entries)
}

fn first_address(entries: &[AccountAddressPoolEntry]) -> Result<Address, String> {
    entries
        .iter()
        .flat_map(|e| e.addresses.iter())
        .find(|info| info.index == 0)
        .map(|info| info.address.clone())
        .ok_or_else(|| "no address at index 0".into())
}

/// The net a seeded record is stored with: no check passes on it, so each
/// one proves this build's accounting rewrote the record.
const UNACCOUNTED_NET: i64 = -1_000_000_007;

/// An incoming Mempool transaction paying `value` to `address`, its record as
/// the wallet would store it (net [`UNACCOUNTED_NET`]), and its coin.
fn incoming(tag: u8, address: &Address, value: u64) -> (TransactionRecord, Utxo) {
    let transaction = Transaction {
        version: 2,
        lock_time: 0,
        input: vec![TxIn {
            previous_output: OutPoint {
                txid: Txid::from_byte_array([tag; 32]),
                vout: 0,
            },
            ..Default::default()
        }],
        output: vec![TxOut {
            value,
            script_pubkey: address.script_pubkey(),
        }],
        special_transaction_payload: None,
    };
    let txout = transaction.output[0].clone();
    let record = TransactionRecord::new(
        transaction,
        bip44(),
        TransactionContext::Mempool,
        TransactionType::Standard,
        TransactionDirection::Incoming,
        Vec::new(),
        vec![OutputDetail {
            index: 0,
            role: OutputRole::Received,
            address: Some(address.clone()),
            value,
        }],
        UNACCOUNTED_NET,
    );
    let outpoint = OutPoint {
        txid: record.txid,
        vout: 0,
    };
    let utxo = Utxo::new(outpoint, txout, address.clone(), 0, false);
    (record, utxo)
}

fn blob<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, String> {
    bincode::serde::encode_to_vec(value, bincode::config::standard()).map_err(|e| e.to_string())
}

fn parse_network(label: &str) -> Result<Network, String> {
    match label {
        "mainnet" => Ok(Network::Mainnet),
        "testnet" => Ok(Network::Testnet),
        "devnet" => Ok(Network::Devnet),
        "regtest" => Ok(Network::Regtest),
        other => Err(format!("unknown network {other}")),
    }
}

struct Seeded {
    wallet: [u8; 32],
    network: Network,
    s_script: Vec<u8>,
    base_total: u64,
    control_coin: (OutPoint, u64),
    s1: TransactionRecord,
}

/// Seed the fork-shaped file before any upstream migration: a retired-only
/// script S (contact xpub A, index 0), an unspent coin at S and the record
/// paying it, written in the pre-V021 row shapes.
fn seed_fork_file(conn: &Connection, obs: &mut Vec<String>) -> Result<Seeded, String> {
    let e = |what: &'static str| move |err: rusqlite::Error| format!("{what}: {err}");
    let (wallet, label): (Vec<u8>, String) = conn
        .query_row("SELECT wallet_id, network FROM wallets", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .map_err(e("template wallet"))?;
    let wallet: [u8; 32] = wallet
        .try_into()
        .map_err(|_| "template wallet id is not 32 bytes".to_string())?;
    let network = parse_network(&label)?;
    let a = contact_xpub(0xA1)?;
    let s_address = first_address(&contact_pools(a, network)?)?;
    let s_script = s_address.script_pubkey().to_bytes();
    let base_total: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(value), 0) FROM core_utxos WHERE wallet_id = ?1 AND spent = 0",
            [&wallet[..]],
            |r| r.get(0),
        )
        .map_err(e("template balance"))?;
    let (control_outpoint, control_value): (Vec<u8>, i64) = conn
        .query_row(
            "SELECT u.outpoint, u.value FROM core_utxos u JOIN core_address_pool p \
               ON p.wallet_id = u.wallet_id AND p.script = u.script \
             WHERE u.wallet_id = ?1 AND u.spent = 0 AND p.account_type = 'standard_bip44' \
               AND p.account_index = 0 ORDER BY u.value DESC LIMIT 1",
            [&wallet[..]],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(e("control coin"))?;
    let control_outpoint: OutPoint =
        bincode::serde::decode_from_slice(&control_outpoint, bincode::config::standard())
            .map_err(|err| format!("control outpoint: {err}"))?
            .0;
    let live: i64 = conn
        .query_row(
            "SELECT count(*) FROM core_address_pool WHERE script = ?1",
            [&s_script],
            |r| r.get(0),
        )
        .map_err(e("S in live pools"))?;
    if live != 0 {
        return Err("sanity: S is in a live pool before seeding".into());
    }
    conn.execute(
        "INSERT INTO core_address_pool_retired (wallet_id, account_type, account_index, key_class, \
           user_identity_id, friend_identity_id, account_xpub, pool_type, address_index, script, used) \
         VALUES (?1, 'dashpay_external', 0, 0, ?2, ?3, ?4, 2, 0, ?5, 1)",
        params![&wallet[..], &OWNER[..], &FRIEND[..], &a.encode()[..], &s_script],
    )
    .map_err(e("seed retired row"))?;
    let (s1, s1_coin) = incoming(0x51, &s_address, V_S1);
    conn.execute(
        "INSERT INTO core_utxos (wallet_id, outpoint, value, script, spent) VALUES (?1, ?2, ?3, ?4, 0)",
        params![&wallet[..], blob(&s1_coin.outpoint)?, V_S1 as i64, &s_script],
    )
    .map_err(e("seed S coin"))?;
    conn.execute(
        "INSERT INTO core_transactions (wallet_id, txid, height, block_hash, block_time, finalized, \
           record_blob) VALUES (?1, ?2, NULL, NULL, NULL, 0, ?3)",
        params![&wallet[..], &s1.txid.to_byte_array()[..], blob(&s1)?],
    )
    .map_err(e("seed S record"))?;
    obs.push(format!(
        "seeded: wallet {} ({label}), S = {} (contact xpub A index 0, retired only), \
         S coin {V_S1} duffs + its record before migration; template balance {base_total}",
        crate::sql::hex(&wallet),
        crate::sql::hex(&s_script)
    ));
    Ok(Seeded {
        wallet,
        network,
        s_script,
        base_total: base_total.max(0) as u64,
        control_coin: (control_outpoint, control_value.max(0) as u64),
        s1,
    })
}

fn s_sanity(conn: &Connection, s: &[u8]) -> Result<(), String> {
    let (retired, live): (i64, i64) = conn
        .query_row(
            "SELECT (SELECT count(*) FROM core_address_pool_retired WHERE script = ?1), \
                    (SELECT count(*) FROM core_address_pool WHERE script = ?1)",
            [s],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| format!("sanity read: {e}"))?;
    if retired != 1 || live != 0 {
        return Err(format!(
            "sanity: S has {retired} retired rows and {live} live rows"
        ));
    }
    Ok(())
}

fn store(
    p: &SqlitePersister,
    wallet: [u8; 32],
    cs: PlatformWalletChangeSet,
    what: &str,
) -> Result<(), String> {
    p.store(wallet, cs)
        .map_err(|e| format!("store {what}: {e:?}"))
}

fn store_incoming(
    p: &SqlitePersister,
    wallet: [u8; 32],
    record: &TransactionRecord,
    utxo: &Utxo,
    what: &str,
) -> Result<(), String> {
    let cs = PlatformWalletChangeSet {
        core: Some(CoreChangeSet {
            records: vec![record.clone()],
            new_utxos: vec![utxo.clone()],
            ..Default::default()
        }),
        ..Default::default()
    };
    store(p, wallet, cs, what)
}

fn in_funds(state: &ClientWalletStartState, outpoint: &OutPoint) -> bool {
    state
        .wallet_info
        .accounts
        .all_funding_accounts()
        .iter()
        .any(|a| a.utxos.contains_key(outpoint))
}

fn net(p: &SqlitePersister, wallet: [u8; 32], txid: &Txid) -> Result<i64, String> {
    p.get_core_tx_record(wallet, txid)
        .map_err(|e| format!("read record {txid}: {e:?}"))?
        .map(|r| r.net_amount)
        .ok_or_else(|| format!("record {txid} not stored"))
}

/// A1–A3: the fork-shaped file through the reconciler's R1 drop and this
/// build's migrations, then a load and three more incoming transactions.
fn run_behaviour(dir: &Path, obs: &mut Vec<String>) -> Result<Checks, String> {
    let path: PathBuf = dir.join("fork.sqlite");
    std::fs::write(&path, PROBE_TEMPLATE).map_err(|e| format!("write probe file: {e}"))?;
    let seeded = {
        let conn = open_existing(&path).map_err(|e| format!("open probe file: {e}"))?;
        if !has_table(&conn, fork::FORK_HISTORY).map_err(|e| e.to_string())? {
            return Err("probe template is not fork-shaped".into());
        }
        let seeded = seed_fork_file(&conn, obs)?;
        crate::reconcile::drop_external_reference(&conn)
            .map_err(|e| format!("reconciler R1 drop: {e}"))?;
        seeded
    };
    let w = seeded.wallet;
    let p = open_persister(dir, &path)?;
    p.load().map_err(|e| format!("first load: {e:?}"))?;
    let b = contact_xpub(0xB2)?;
    let b_pools = contact_pools(b, seeded.network)?;
    let contact_address = first_address(&b_pools)?;
    store(
        &p,
        w,
        PlatformWalletChangeSet {
            account_registrations: vec![AccountRegistrationEntry {
                account_type: contact_account(),
                account_xpub: b,
            }],
            account_address_pools: b_pools,
            ..Default::default()
        },
        "contact B registration",
    )?;
    let live_script: Vec<u8> = {
        let conn = open_existing(&path).map_err(|e| format!("side read: {e}"))?;
        conn.query_row(
            "SELECT script FROM core_address_pool WHERE wallet_id = ?1 AND account_type = \
               'standard_bip44' AND account_index = 0 AND pool_type = 0 AND used = 0 \
             ORDER BY address_index LIMIT 1",
            [&w[..]],
            |r| r.get(0),
        )
        .map_err(|e| format!("live address: {e}"))?
    };
    let live_address = Address::from_script(&ScriptBuf::from_bytes(live_script), seeded.network)
        .map_err(|e| format!("live address: {e}"))?;
    let s_address = Address::from_script(
        &ScriptBuf::from_bytes(seeded.s_script.clone()),
        seeded.network,
    )
    .map_err(|e| format!("S address: {e}"))?;
    let (t_live, t_live_coin) = incoming(0x61, &live_address, V_LIVE);
    let (t_contact, t_contact_coin) = incoming(0x62, &contact_address, V_CONTACT);
    let (t_s2, t_s2_coin) = incoming(0x63, &s_address, V_S2);
    store_incoming(&p, w, &t_live, &t_live_coin, "T_live")?;
    store_incoming(&p, w, &t_contact, &t_contact_coin, "T_contact")?;
    store_incoming(&p, w, &t_s2, &t_s2_coin, "T_S2")?;
    drop(p);
    {
        let conn = open_existing(&path).map_err(|e| format!("sanity open: {e}"))?;
        s_sanity(&conn, &seeded.s_script)?;
    }
    obs.push("sanity before reopen: S has one retired row and no live row".into());

    let p = open_persister(dir, &path)?;
    let state = p.load().map_err(|e| format!("reload: {e:?}"))?;
    let wallet = state
        .wallets
        .get(&w)
        .ok_or_else(|| "wallet not rehydrated".to_string())?;
    let mut checks = Checks::default();
    let mut control = |ok: bool, what: String| {
        obs.push(format!(
            "control {}: {what}",
            if ok { "ok" } else { "FAILED" }
        ));
        if !ok {
            checks.controls_failed.push(what);
        }
    };
    let account0 = wallet.wallet_info.accounts.standard_bip44_accounts.get(&0);
    let (c1_outpoint, c1_value) = seeded.control_coin;
    let c1 = account0
        .and_then(|a| a.utxos.get(&c1_outpoint))
        .map(|u| u.value());
    control(
        c1 == Some(c1_value),
        format!("C1 fixture BIP44 coin in account 0 with {c1_value}: {c1:?}"),
    );
    let live_net = net(&p, w, &t_live.txid)?;
    let live_in_0 = account0.is_some_and(|a| a.utxos.contains_key(&t_live_coin.outpoint));
    control(
        live_net == V_LIVE as i64 && live_in_0,
        format!("C2 T_live net {live_net} (want {V_LIVE}), coin in account 0: {live_in_0}"),
    );
    let contact_net = net(&p, w, &t_contact.txid)?;
    let contact_in_funds = in_funds(wallet, &t_contact_coin.outpoint);
    control(
        contact_net == 0 && !contact_in_funds,
        format!(
            "C3 T_contact net {contact_net} (want 0), coin in a funds account: {contact_in_funds}"
        ),
    );
    let mut retired = |ok: bool, what: String| {
        obs.push(format!(
            "retired check {}: {what}",
            if ok { "ok" } else { "FAILED" }
        ));
        if !ok {
            checks.retired_failed.push(what);
        }
    };
    let s1_net = net(&p, w, &seeded.s1.txid)?;
    retired(
        s1_net == 0,
        format!("S1 record net after V021 replay {s1_net} (want 0)"),
    );
    let s2_net = net(&p, w, &t_s2.txid)?;
    retired(s2_net == 0, format!("T_S2 net {s2_net} (want 0)"));
    let s1_outpoint = OutPoint {
        txid: seeded.s1.txid,
        vout: 0,
    };
    let s1_in = in_funds(wallet, &s1_outpoint);
    let s2_in = in_funds(wallet, &t_s2_coin.outpoint);
    retired(
        !s1_in && !s2_in,
        format!("S coins in a funds account: S1 {s1_in}, S2 {s2_in}"),
    );
    let balance = wallet.wallet_info.balance;
    let want = seeded.base_total + V_LIVE;
    retired(
        balance.total() == want,
        format!(
            "balance total {} (want {want}; confirmed {}, unconfirmed {}, immature {}, locked {})",
            balance.total(),
            balance.confirmed(),
            balance.unconfirmed(),
            balance.immature(),
            balance.locked()
        ),
    );
    let in_pool = wallet.wallet_info.all_managed_accounts().iter().any(|m| {
        m.managed_account_type().address_pools().iter().any(|pool| {
            pool.addresses()
                .values()
                .any(|info| info.address.script_pubkey().as_bytes() == seeded.s_script)
        })
    });
    retired(
        !in_pool,
        format!("S in a loaded account's pool or used set: {in_pool}"),
    );
    drop(p);
    {
        let conn = open_existing(&path).map_err(|e| format!("final open: {e}"))?;
        s_sanity(&conn, &seeded.s_script)?;
    }
    Ok(checks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adopters(p9a: P9aStatus) -> Adopters {
        Adopters {
            embedded: Vec::new(),
            fresh: Vec::new(),
            p9a,
            observations: Vec::new(),
        }
    }

    #[test]
    fn p9a_present_only_for_present() {
        assert!(p9a_present(&adopters(P9aStatus::Present)));
        assert!(!p9a_present(&adopters(P9aStatus::Absent)));
        assert!(!p9a_present(&adopters(P9aStatus::Broken {
            detail: "x".into()
        })));
        assert!(!r1d_line_required(&adopters(P9aStatus::Present)));
        assert!(r1d_line_required(&adopters(P9aStatus::Absent)));
    }

    fn checks(controls: &[&str], retired: &[&str]) -> Result<Checks, String> {
        Ok(Checks {
            controls_failed: controls.iter().map(|s| s.to_string()).collect(),
            retired_failed: retired.iter().map(|s| s.to_string()).collect(),
        })
    }

    #[test]
    fn decide_maps_schema_and_behaviour_to_three_outcomes() {
        assert_eq!(decide(true, checks(&[], &[])), P9aStatus::Present);
        assert_eq!(decide(false, checks(&[], &["S1 net"])), P9aStatus::Absent);
        assert_eq!(decide(false, checks(&[], &[])), P9aStatus::Absent);
        assert!(matches!(
            decide(true, checks(&[], &["S1 net"])),
            P9aStatus::Broken { .. }
        ));
        assert!(matches!(
            decide(true, checks(&["C1"], &[])),
            P9aStatus::Broken { .. }
        ));
        assert!(matches!(
            decide(false, checks(&["C2"], &[])),
            P9aStatus::Broken { .. }
        ));
        assert!(matches!(
            decide(true, Err("io".into())),
            P9aStatus::Broken { .. }
        ));
        assert!(matches!(
            decide(false, Err("io".into())),
            P9aStatus::Broken { .. }
        ));
    }
}
