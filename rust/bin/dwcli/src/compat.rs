//! R2 commands: Dash Core file import and export, `.dwbackup` backups, PSBT
//! and address listing. Output is line-oriented `key value` text for the
//! regtest `restore` suite (`regtest/harness/tests/test_restore.py`).
//! Secrets are read from files, never from argv.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Subcommand;
use dw_engine::{
    AddressChain, AddressFilter, CoreExportFormat, Engine, ImportOptions, ImportReport,
    KeyMaterial, NetworkSession, WalletId,
};
use dw_vault::GrantPurpose;
use zeroize::Zeroizing;

use crate::pay::{credential, recipient, wait_for_height, wallet_id};

#[derive(Subcommand)]
pub enum CompatCommand {
    /// Print what a file is (dumpwallet, wallet.dat, .dwbackup, PSBT).
    Inspect { path: PathBuf },
    /// Import a Dash Core `dumpwallet` file.
    ImportDump {
        path: PathBuf,
        #[command(flatten)]
        opts: ImportArgs,
    },
    /// Import a Dash Core SQLite descriptor `wallet.dat`.
    ImportWalletDat {
        path: PathBuf,
        /// File holding the wallet.dat passphrase (first line, bytes as is).
        #[arg(long)]
        walletpass_file: Option<PathBuf>,
        #[command(flatten)]
        opts: ImportArgs,
    },
    /// Import `listdescriptors true` JSON (file) or an HD seed (hex in a file).
    ImportKey {
        #[arg(long, conflicts_with = "hdseed_file")]
        descriptors_file: Option<PathBuf>,
        #[arg(long)]
        hdseed_file: Option<PathBuf>,
        #[command(flatten)]
        opts: ImportArgs,
    },
    /// Write the wallet for dash-qt: `dumpwallet` or `importdescriptors` JSON.
    ExportCore {
        wallet: String,
        #[arg(long, value_parser = ["dumpwallet", "descriptors"])]
        format: String,
        dest: PathBuf,
    },
    /// Whether phrase + passphrase + `upgradetohd` rebuild the wallet in dash-qt.
    CoreCompat { wallet: String },
    /// Print the recovery phrase and passphrase (hex) for a test driver.
    RevealMnemonic { wallet: String },
    /// Print the first `count` addresses of a chain: `addr <index> <address>`.
    Addresses {
        wallet: String,
        #[arg(long, value_parser = ["receive", "change"], default_value = "receive")]
        chain: String,
        #[arg(long, default_value_t = 20)]
        count: usize,
    },
    /// Write a `.dwbackup` of the wallet.
    Backup {
        wallet: String,
        dest: PathBuf,
        /// Backup passphrase file (unencrypted vaults only).
        #[arg(long)]
        backup_pass_file: Option<PathBuf>,
    },
    /// Restore a `.dwbackup`.
    Restore {
        path: PathBuf,
        #[arg(long)]
        backup_pass_file: Option<PathBuf>,
    },
    /// List automatic backups.
    Backups { wallet: Option<String> },
    /// dash-qt "Create Unsigned": print the PSBT (base64) paying `--to`.
    PsbtCreate {
        wallet: String,
        #[arg(long = "to", required = true)]
        to: Vec<String>,
        #[arg(long)]
        sync_height: Option<u32>,
    },
    /// Print the PSBT Operations dialog lines for a PSBT file (base64 or binary).
    PsbtAnalyze {
        path: PathBuf,
        #[arg(long)]
        wallet: Option<String>,
    },
    /// Sign a PSBT file with the wallet; print the signed PSBT (base64).
    PsbtSign {
        wallet: String,
        path: PathBuf,
        /// Grant cap in duffs (default: the PSBT's external outputs).
        #[arg(long)]
        max_duffs: Option<u64>,
    },
    /// Finalize and broadcast a complete PSBT file; print the txid.
    PsbtBroadcast {
        path: PathBuf,
        #[arg(long, default_value_t = 0)]
        sync_height: u32,
        #[arg(long)]
        wallet: Option<String>,
    },
}

#[derive(clap::Args)]
pub struct ImportArgs {
    #[arg(long)]
    birth_height: Option<u32>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    lookahead: Option<u32>,
}

impl ImportArgs {
    fn options(self) -> ImportOptions {
        ImportOptions {
            birth_height: self.birth_height,
            core_compat: true,
            name: self.name,
            lookahead: self.lookahead,
        }
    }
}

fn first_line(path: &std::path::Path) -> Result<Zeroizing<Vec<u8>>, String> {
    crate::read_passphrase(path)
}

fn print_report(r: &ImportReport) {
    println!(
        "wallet_id {} labels={} keys_not_imported={} scripts_not_imported={} core_compat_seed={}",
        r.wallet_id,
        r.labels_imported,
        r.keys_not_imported,
        r.scripts_not_imported,
        u8::from(r.core_compat_seed)
    );
}

fn read_psbt(path: &PathBuf) -> Result<dw_psbt::PartiallySignedTransaction, String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    dw_psbt::parse(&data).map_err(|e| e.to_string())
}

fn grant(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    passphrase: Option<&Zeroizing<Vec<u8>>>,
    purpose: GrantPurpose,
    id: WalletId,
) -> Result<String, String> {
    let pw = passphrase.cloned();
    let s = Arc::clone(session);
    engine
        .block_on(
            session
                .vault_op(move |v| v.authorize(purpose, Some(&id.0), credential(&s, pw.as_ref()))),
        )
        .map(|g| g.id)
        .map_err(|e| e.to_string())
}

pub fn run(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    passphrase: Option<&Zeroizing<Vec<u8>>>,
    cmd: CompatCommand,
) -> Result<(), String> {
    let e = |e: dw_engine::EngineError| e.to_string();
    match cmd {
        CompatCommand::Inspect { path } => {
            println!(
                "kind {:?}",
                engine
                    .block_on(engine.inspect_wallet_file(path))
                    .map_err(e)?
            );
        }
        CompatCommand::ImportDump { path, opts } => {
            let r = engine
                .block_on(session.import_dump_wallet(path, opts.options()))
                .map_err(e)?;
            print_report(&r);
        }
        CompatCommand::ImportWalletDat {
            path,
            walletpass_file,
            opts,
        } => {
            let pass = walletpass_file
                .as_ref()
                .map(|p| first_line(p))
                .transpose()?;
            let r = engine
                .block_on(session.import_wallet_dat(path, pass, opts.options()))
                .map_err(e)?;
            print_report(&r);
        }
        CompatCommand::ImportKey {
            descriptors_file,
            hdseed_file,
            opts,
        } => {
            let material = match (descriptors_file, hdseed_file) {
                (Some(p), None) => KeyMaterial::Descriptors(Zeroizing::new(
                    std::fs::read(&p).map_err(|e| e.to_string())?,
                )),
                (None, Some(p)) => {
                    let hex_text = first_line(&p)?;
                    let text = std::str::from_utf8(&hex_text).map_err(|e| e.to_string())?;
                    KeyMaterial::HdSeed(Zeroizing::new(
                        hex::decode(text.trim()).map_err(|e| e.to_string())?,
                    ))
                }
                _ => return Err("give --descriptors-file or --hdseed-file".into()),
            };
            let r = engine
                .block_on(session.import_key_material(material, opts.options()))
                .map_err(e)?;
            print_report(&r);
        }
        CompatCommand::ExportCore {
            wallet,
            format,
            dest,
        } => {
            let id = wallet_id(&wallet)?;
            let format = if format == "dumpwallet" {
                CoreExportFormat::DumpWallet
            } else {
                CoreExportFormat::ImportDescriptorsJson
            };
            let g = grant(engine, session, passphrase, GrantPurpose::RevealSecret, id)?;
            let r = engine
                .block_on(session.export_for_core(id, format, dest, g))
                .map_err(e)?;
            println!(
                "exported {} keys={} warnings={:?}",
                r.path.display(),
                r.key_count,
                r.warnings
            );
        }
        CompatCommand::CoreCompat { wallet } => {
            let c = engine
                .block_on(session.core_mnemonic_compatibility(wallet_id(&wallet)?))
                .map_err(e)?;
            println!(
                "core_compatible {} warnings={:?}",
                u8::from(c.core_compatible),
                c.warnings
            );
        }
        CompatCommand::RevealMnemonic { wallet } => {
            let id = wallet_id(&wallet)?;
            let g = grant(engine, session, passphrase, GrantPurpose::RevealSecret, id)?;
            let m = engine
                .block_on(session.vault_op(move |v| v.reveal_mnemonic(&id.0, &g)))
                .map_err(e)?;
            // Hex keeps any byte of the passphrase intact for the driver.
            println!("mnemonic_hex {}", hex::encode(&m.phrase[..]));
            println!("passphrase_hex {}", hex::encode(&m.bip39_passphrase[..]));
        }
        CompatCommand::Addresses {
            wallet,
            chain,
            count,
        } => {
            let chain = if chain == "change" {
                AddressChain::Change
            } else {
                AddressChain::Receiving
            };
            let mut list = engine
                .block_on(session.addresses(
                    wallet_id(&wallet)?,
                    AddressFilter {
                        chain: Some(chain),
                        used: None,
                    },
                ))
                .map_err(e)?;
            list.sort_by_key(|a| a.index);
            if list.len() < count {
                return Err(format!("only {} addresses derived", list.len()));
            }
            for a in list.into_iter().take(count) {
                println!("addr {} {}", a.index, a.address);
            }
        }
        CompatCommand::Backup {
            wallet,
            dest,
            backup_pass_file,
        } => {
            let pass = backup_pass_file
                .as_ref()
                .map(|p| first_line(p))
                .transpose()?;
            let info = engine
                .block_on(session.backup_wallet(wallet_id(&wallet)?, dest, pass))
                .map_err(e)?;
            println!("backup {} size={}", info.path.display(), info.size_bytes);
        }
        CompatCommand::Restore {
            path,
            backup_pass_file,
        } => {
            let pass = backup_pass_file
                .as_ref()
                .map(|p| first_line(p))
                .transpose()?
                .or_else(|| passphrase.cloned());
            for id in engine
                .block_on(session.restore_backup(path, pass))
                .map_err(e)?
            {
                println!("wallet_id {id}");
            }
        }
        CompatCommand::Backups { wallet } => {
            let id = wallet.as_deref().map(wallet_id).transpose()?;
            for b in engine.block_on(session.automatic_backups(id)).map_err(e)? {
                println!(
                    "backup {} {} {}",
                    b.wallet_id,
                    b.created_at,
                    b.path.display()
                );
            }
        }
        CompatCommand::PsbtCreate {
            wallet,
            to,
            sync_height,
        } => {
            let id = wallet_id(&wallet)?;
            if let Some(h) = sync_height {
                wait_for_height(engine, session, id, h, Duration::from_secs(180))?;
            }
            let draft = session.new_tx_draft(id).map_err(e)?;
            draft
                .set_recipients(
                    to.iter()
                        .map(|r| recipient(r, &None, &None))
                        .collect::<Result<_, _>>()?,
                )
                .map_err(e)?;
            let psbt = engine.block_on(draft.create_unsigned()).map_err(e)?;
            println!("psbt {}", dw_psbt::to_base64(&psbt));
        }
        CompatCommand::PsbtAnalyze { path, wallet } => {
            let psbt = read_psbt(&path)?;
            let id = wallet.as_deref().map(wallet_id).transpose()?;
            let a = engine.block_on(session.analyze_psbt(id, psbt)).map_err(e)?;
            for o in &a.outputs {
                println!(
                    "output {} {} mine={}",
                    o.address.as_deref().unwrap_or("-"),
                    o.amount,
                    u8::from(o.is_mine)
                );
            }
            println!(
                "fee {} total {} unsigned_inputs={} status={:?} signability={:?}",
                a.fee.map_or("-".into(), |f| f.to_string()),
                a.total.map_or("-".into(), |t| t.to_string()),
                a.unsigned_inputs,
                a.status,
                a.signability
            );
        }
        CompatCommand::PsbtSign {
            wallet,
            path,
            max_duffs,
        } => {
            let id = wallet_id(&wallet)?;
            let psbt = read_psbt(&path)?;
            let cap = match max_duffs {
                Some(c) => c,
                None => engine
                    .block_on(session.analyze_psbt(Some(id), psbt.clone()))
                    .map_err(e)?
                    .external_sent
                    .unwrap_or(0),
            };
            let g = grant(
                engine,
                session,
                passphrase,
                GrantPurpose::Spend { max_duffs: cap },
                id,
            )?;
            let signed = engine.block_on(session.sign_psbt(id, psbt, g)).map_err(e)?;
            println!("psbt {}", dw_psbt::to_base64(&signed));
        }
        CompatCommand::PsbtBroadcast {
            path,
            sync_height,
            wallet,
        } => {
            let psbt = read_psbt(&path)?;
            if let Some(w) = wallet {
                wait_for_height(
                    engine,
                    session,
                    wallet_id(&w)?,
                    sync_height,
                    Duration::from_secs(180),
                )?;
            }
            let txid = engine.block_on(session.broadcast_psbt(psbt)).map_err(e)?;
            println!("txid {txid}");
        }
    }
    Ok(())
}
