//! M2 R1 commands: wallet load/unload and watch-only wallets, account
//! xpubs, rescans, abandon/resend, transaction extras and notices, CSV
//! export, fee policy, the coin-control summary, Tools information and the
//! console. Output is line-oriented `key=value` text for the regtest `l2-tools`
//! suite (`regtest/harness/tests/test_l2_tools.py`).

use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Subcommand;
use dw_console::{ConsoleContext, ConsoleFailure};
use dw_engine::{
    Engine, EngineError, FeeMode, HistoryFilter, HistorySort, NetworkSession, RescanFrom,
    WatchOnlyOptions,
};
use dw_vault::GrantPurpose;
use zeroize::Zeroizing;

use crate::pay::{credential, outpoint, wait_for_height, wallet_id};

#[derive(Subcommand)]
pub enum ToolsCommand {
    /// Every registered wallet: `wallet <id> loaded=<0|1> startup=<0|1>
    /// watch_only=<0|1> name=<name>`.
    LoadStates,
    /// dash-qt Close Wallet.
    Unload {
        wallet: String,
        /// Also remove it from the load-on-startup list (dash-qt does).
        #[arg(long)]
        keep_startup: bool,
    },
    /// dash-qt Open Wallet.
    Load { wallet: String },
    /// Register a watch-only wallet from a BIP44 account xpub; prints
    /// `wallet_id <id>`.
    WatchOnly {
        xpub: String,
        #[arg(long)]
        birth_height: Option<u32>,
        #[arg(long)]
        name: Option<String>,
    },
    /// Print the wallet's BIP44 account xpub: `xpub <key> path=<path>`.
    Xpub {
        wallet: String,
        #[arg(long, default_value_t = 0)]
        account: u32,
    },
    /// Sync to `--sync-height`, rescan, wait until the rescan finished and
    /// every wallet is back at the tip; print the wallets.
    Rescan {
        #[arg(long)]
        sync_height: u32,
        /// First block to rescan; omit for each wallet's birth height.
        #[arg(long)]
        from_height: Option<u32>,
        #[arg(long, default_value_t = 240)]
        timeout_secs: u64,
    },
    /// Set the wallet's birth height.
    BirthHeight { wallet: String, height: u32 },
    /// Print `tx_detail_extras`: `extras key=value…`.
    Extras { wallet: String, txid: String },
    /// Sync to `--sync-height`, abandon the transaction, wait until the
    /// wallet is back at the tip (the abandon's rescan), print the coins.
    Abandon {
        wallet: String,
        txid: String,
        #[arg(long)]
        sync_height: u32,
        #[arg(long, default_value_t = 240)]
        timeout_secs: u64,
    },
    /// Sync to `--sync-height`, resend the transaction, keep SPV running for
    /// `--linger-secs` so the send goes out, print `resent <txid>`.
    Resend {
        wallet: String,
        txid: String,
        #[arg(long)]
        sync_height: u32,
        #[arg(long, default_value_t = 10)]
        linger_secs: u64,
    },
    /// Print the CSV export of the wallet's whole history.
    ExportCsv { wallet: String },
    /// Print `notice` rows for the txids.
    Notices { wallet: String, txids: Vec<String> },
    /// Print the fee policy: `policy source=… min=…` and `target blocks=n
    /// rate=r` rows.
    Fees,
    /// Print the coin-control summary of `--coin` outpoints paying `--pay`
    /// amounts.
    Summary {
        wallet: String,
        #[arg(long = "coin")]
        coins: Vec<String>,
        #[arg(long = "pay")]
        pay: Vec<u64>,
        #[arg(long)]
        fee_per_kb: Option<u64>,
    },
    /// Print node information (and warnings) while synced to
    /// `--sync-height` when given.
    NodeInfo {
        #[arg(long)]
        sync_height: Option<u32>,
    },
    /// Run one console line. With `--sync-height` SPV runs and is synced
    /// first. A command that needs authorization is authorized with the
    /// passphrase file (or none for an unencrypted vault) and run again.
    Console {
        line: String,
        #[arg(long)]
        wallet: Option<String>,
        #[arg(long)]
        sync_height: Option<u32>,
        /// Do not authorize; report `authorization_required` instead.
        #[arg(long)]
        no_grant: bool,
    },
    /// Sync to `--sync-height` while printing every `NewTransactions`
    /// event as `newtx wallet=<id> count=<n> catch_up=<0|1> txids=<a,b>`.
    WatchEvents {
        #[arg(long)]
        sync_height: u32,
        /// Keep listening this long after the height is reached.
        #[arg(long, default_value_t = 2)]
        linger_secs: u64,
        #[arg(long, default_value_t = 240)]
        timeout_secs: u64,
    },
}

fn txid(s: &str) -> Result<dashcore::Txid, String> {
    dashcore::Txid::from_str(s).map_err(|e| format!("bad txid {s:?}: {e}"))
}

fn flag(b: bool) -> u8 {
    u8::from(b)
}

/// Starts SPV (if needed) and waits until every loaded wallet scanned
/// `height`.
fn sync_all(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    height: u32,
    timeout: Duration,
) -> Result<(), String> {
    let ids: Vec<_> = session
        .wallet_infos()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|w| w.wallet_id)
        .collect();
    if ids.is_empty() {
        if !session.spv_running().map_err(|e| e.to_string())? {
            engine
                .block_on(session.start_spv())
                .map_err(|e| e.to_string())?;
        }
        return wait_tip(session, height, timeout);
    }
    for id in ids {
        wait_for_height(engine, session, id, height, timeout)?;
    }
    // Stored scan heights are known before SPV reports; wait for SPV too.
    wait_tip(session, height, timeout)
}

/// Waits until SPV's header tip reaches `height`.
fn wait_tip(session: &NetworkSession, height: u32, timeout: Duration) -> Result<(), String> {
    let start = Instant::now();
    loop {
        let tip = session
            .sync_snapshot()
            .map_err(|e| e.to_string())?
            .tip_height
            .unwrap_or(0);
        if tip >= height {
            return Ok(());
        }
        if start.elapsed() > timeout {
            return Err(format!("tip {tip} did not reach {height} in {timeout:?}"));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn print_coins(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: dw_engine::WalletId,
) -> Result<(), String> {
    let coins = engine
        .block_on(session.utxos(
            id,
            dw_engine::CoinFilter {
                include_locked: true,
                ..Default::default()
            },
        ))
        .map_err(|e| e.to_string())?;
    for c in coins {
        println!(
            "utxo {} {} conf={} reserved={}",
            c.outpoint,
            c.amount,
            c.confirmations,
            flag(c.reserved)
        );
    }
    if let Some(b) = session.balances(&id).map_err(|e| e.to_string())? {
        println!(
            "balance confirmed={} unconfirmed={} total={}",
            b.confirmed, b.unconfirmed, b.total
        );
    }
    Ok(())
}

pub fn run(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    passphrase: Option<&Zeroizing<Vec<u8>>>,
    cmd: ToolsCommand,
) -> Result<(), String> {
    let e = |e: EngineError| e.to_string();
    match cmd {
        ToolsCommand::LoadStates => {
            for w in session.wallet_load_states().map_err(e)? {
                println!(
                    "wallet {} loaded={} startup={} watch_only={} name={}",
                    w.wallet_id,
                    flag(w.loaded),
                    flag(w.load_on_startup),
                    flag(w.watch_only),
                    w.name
                );
            }
        }
        ToolsCommand::Unload {
            wallet,
            keep_startup,
        } => {
            let id = wallet_id(&wallet)?;
            engine.block_on(session.unload_wallet(id)).map_err(e)?;
            if !keep_startup {
                engine
                    .block_on(session.set_load_on_startup(id, false))
                    .map_err(e)?;
            }
            println!("unloaded {id}");
        }
        ToolsCommand::Load { wallet } => {
            let id = wallet_id(&wallet)?;
            engine.block_on(session.load_wallet(id)).map_err(e)?;
            engine
                .block_on(session.set_load_on_startup(id, true))
                .map_err(e)?;
            println!("loaded {id}");
        }
        ToolsCommand::WatchOnly {
            xpub,
            birth_height,
            name,
        } => {
            let id = engine
                .block_on(session.import_watch_only(
                    xpub,
                    WatchOnlyOptions {
                        name,
                        birth_height,
                        lookahead: None,
                    },
                ))
                .map_err(e)?;
            println!("wallet_id {id}");
        }
        ToolsCommand::Xpub { wallet, account } => {
            let x = engine
                .block_on(session.account_xpub(wallet_id(&wallet)?, account))
                .map_err(e)?;
            println!("xpub {} path={}", x.xpub, x.derivation_path);
        }
        ToolsCommand::Rescan {
            sync_height,
            from_height,
            timeout_secs,
        } => {
            let timeout = Duration::from_secs(timeout_secs);
            sync_all(engine, session, sync_height, timeout)?;
            let from = match from_height {
                Some(0) => RescanFrom::Genesis,
                Some(h) => RescanFrom::Height(h),
                None => RescanFrom::WalletBirth,
            };
            engine.block_on(session.rescan(from)).map_err(e)?;
            let p = session.rescan_progress().map_err(e)?;
            println!(
                "rescan started from={}",
                p.as_ref()
                    .map_or("none".to_string(), |p| p.from_height.to_string())
            );
            let start = Instant::now();
            let mut last = String::new();
            while let Some(p) = session.rescan_progress().map_err(e)? {
                let line = format!(
                    "rescan at={:?} target={:?} scan={:?}",
                    p.current_height,
                    p.target_height,
                    session
                        .wallet_infos()
                        .map_err(e)?
                        .iter()
                        .map(|w| session.wallet_scan_height(&w.wallet_id))
                        .collect::<Vec<_>>()
                );
                if line != last {
                    eprintln!("{line}");
                    last = line;
                }
                if start.elapsed() > timeout {
                    return Err(format!("rescan did not finish in time: {last}"));
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            sync_all(engine, session, sync_height, timeout)?;
            println!("rescan done");
            for w in session.wallet_infos().map_err(e)? {
                let total = w.balances.map_or("unknown".into(), |b| b.total.to_string());
                println!("wallet {} total={total}", w.wallet_id);
            }
        }
        ToolsCommand::BirthHeight { wallet, height } => {
            engine
                .block_on(session.set_birth_height(wallet_id(&wallet)?, height))
                .map_err(e)?;
            println!("birth_height {height}");
        }
        ToolsCommand::Extras { wallet, txid: t } => {
            let x = engine
                .block_on(session.tx_detail_extras(wallet_id(&wallet)?, txid(&t)?))
                .map_err(e)?;
            println!(
                "extras txid={} coinbase={} credit={} debit={} net={} in_mempool={} abandoned={} can_abandon={} can_resend={} dust_locked={}",
                x.txid,
                flag(x.is_coinbase),
                x.total_credit,
                x.total_debit.map_or("unknown".into(), |d| d.to_string()),
                x.net,
                x.in_mempool
                    .map_or("unknown".into(), |m| flag(m).to_string()),
                flag(x.abandoned),
                flag(x.can_abandon),
                flag(x.can_resend),
                x.dust_locked_outputs.len()
            );
        }
        ToolsCommand::Abandon {
            wallet,
            txid: t,
            sync_height,
            timeout_secs,
        } => {
            let id = wallet_id(&wallet)?;
            let timeout = Duration::from_secs(timeout_secs);
            sync_all(engine, session, sync_height, timeout)?;
            match engine.block_on(session.abandon_transaction(id, txid(&t)?)) {
                Ok(()) => println!("abandoned {t}"),
                Err(EngineError::TxActionRefused(r)) => {
                    println!("refused {r:?}");
                    return Ok(());
                }
                Err(other) => return Err(other.to_string()),
            }
            // The abandon rewound the scan to bring the spent coins back.
            wait_for_height(engine, session, id, sync_height, timeout)?;
            print_coins(engine, session, id)?;
        }
        ToolsCommand::Resend {
            wallet,
            txid: t,
            sync_height,
            linger_secs,
        } => {
            let id = wallet_id(&wallet)?;
            // SPV must be connected, not only the stored heights reached.
            sync_all(engine, session, sync_height, Duration::from_secs(240))?;
            match engine.block_on(session.resend_transaction(id, txid(&t)?)) {
                Ok(()) => println!("resent {t}"),
                Err(EngineError::TxActionRefused(r)) => {
                    println!("refused {r:?}");
                    return Ok(());
                }
                Err(other) => return Err(other.to_string()),
            }
            std::thread::sleep(Duration::from_secs(linger_secs));
        }
        ToolsCommand::ExportCsv { wallet } => {
            let csv = engine
                .block_on(session.export_history_csv(
                    wallet_id(&wallet)?,
                    HistoryFilter::default(),
                    HistorySort::NewestFirst,
                    dw_units::Unit::Dash,
                    Vec::new(),
                    0,
                ))
                .map_err(e)?;
            print!("{csv}");
        }
        ToolsCommand::Notices { wallet, txids } => {
            let txids = txids
                .iter()
                .map(|t| txid(t))
                .collect::<Result<Vec<_>, _>>()?;
            for n in engine
                .block_on(session.tx_notices(wallet_id(&wallet)?, txids))
                .map_err(e)?
            {
                println!(
                    "notice txid={} index={} amount={} type={:?} coinjoin_internal={} address={}",
                    n.txid,
                    n.record_index,
                    n.amount,
                    n.tx_type,
                    flag(n.coinjoin_internal),
                    n.address.as_deref().unwrap_or("-")
                );
            }
        }
        ToolsCommand::Fees => {
            let p = dw_engine::fee_policy();
            println!(
                "policy source={:?} min={} max_custom={} max_fee={} max_rate={}",
                p.source,
                p.min_relay_per_kb,
                p.max_custom_per_kb,
                p.max_tx_fee,
                p.max_broadcast_rate_per_kb
            );
            for t in p.targets {
                println!("target blocks={} rate={}", t.target_blocks, t.duffs_per_kb);
            }
        }
        ToolsCommand::Summary {
            wallet,
            coins,
            pay,
            fee_per_kb,
        } => {
            let list = coins
                .iter()
                .map(|o| outpoint(o))
                .collect::<Result<Vec<_>, _>>()?;
            let fee = match fee_per_kb {
                Some(r) => FeeMode::PerKb(r),
                None => FeeMode::Recommended { target_blocks: 6 },
            };
            let s = engine
                .block_on(session.coin_selection_summary(
                    wallet_id(&wallet)?,
                    list,
                    pay,
                    fee,
                    false,
                ))
                .map_err(e)?;
            println!(
                "summary quantity={} amount={} bytes={} fee={} after_fee={} change={} change_to_fee={} insufficient={} unavailable={}",
                s.quantity,
                s.amount,
                s.bytes,
                s.fee,
                s.after_fee,
                s.change,
                flag(s.change_to_fee),
                flag(s.insufficient_funds),
                s.unavailable.len()
            );
        }
        ToolsCommand::NodeInfo { sync_height } => {
            if let Some(h) = sync_height {
                sync_all(engine, session, h, Duration::from_secs(240))?;
            }
            let i = session.node_info().map_err(e)?;
            println!(
                "info agent={} peers_out={} peers_in={} tip={} mempool={} masternodes={}",
                i.user_agent,
                i.connections_out,
                i.connections_in,
                i.tip_height.map_or("unknown".into(), |h| h.to_string()),
                i.mempool_tx_count
                    .map_or("unknown".into(), |m| m.to_string()),
                i.masternodes
                    .map_or("unknown".into(), |m| format!("{}/{}", m.enabled, m.total))
            );
            for w in session.warnings().map_err(e)? {
                println!("warning {:?}", w.code);
            }
        }
        ToolsCommand::Console {
            line,
            wallet,
            sync_height,
            no_grant,
        } => {
            if let Some(h) = sync_height {
                sync_all(engine, session, h, Duration::from_secs(240))?;
            }
            let wallet = wallet.as_deref().map(wallet_id).transpose()?;
            let mut ctx = ConsoleContext {
                session: Arc::clone(session),
                wallet,
                grant_id: None,
            };
            let mut outcome = engine.block_on(ctx.run(&line));
            if let Err(ConsoleFailure::AuthorizationRequired { purpose, wallet }) = &outcome
                && !no_grant
            {
                let grant = session
                    .vault()
                    .authorize(
                        *purpose,
                        wallet.as_ref().map(|w| &w.0),
                        credential(session, passphrase),
                    )
                    .map_err(|e| e.to_string())?;
                ctx.grant_id = Some(grant.id);
                outcome = engine.block_on(ctx.run(&line));
            }
            println!(
                "history {}",
                dw_console::redact(&line).unwrap_or_else(|_| "<unparsable>".into())
            );
            match outcome {
                Ok(p) => {
                    println!("result json={}", flag(p.is_json));
                    println!("{}", p.result);
                }
                Err(ConsoleFailure::Rpc { code, message }) => {
                    println!("error {message} (code {code})")
                }
                Err(ConsoleFailure::NotAvailable(c)) => println!("not_available {c}"),
                Err(ConsoleFailure::AuthorizationRequired { purpose, .. }) => {
                    let kind = match purpose {
                        GrantPurpose::Spend { .. } => "spend",
                        GrantPurpose::SignMessage => "sign_message",
                        _ => "other",
                    };
                    println!("authorization_required {kind}")
                }
                Err(ConsoleFailure::WalletRequired) => println!("wallet_required"),
                Err(ConsoleFailure::Parse(d)) => println!("parse_error {d}"),
                Err(ConsoleFailure::Engine(err)) => println!("engine_error {}", err.code()),
            }
        }
        // main's event sink prints the `newtx` lines while this waits.
        ToolsCommand::WatchEvents {
            sync_height,
            linger_secs,
            timeout_secs,
        } => {
            sync_all(
                engine,
                session,
                sync_height,
                Duration::from_secs(timeout_secs),
            )?;
            std::thread::sleep(Duration::from_secs(linger_secs));
            println!("watched {sync_height}");
        }
    }
    Ok(())
}
