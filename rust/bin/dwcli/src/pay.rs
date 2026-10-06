//! E2 commands: sync wait, funding address, coin control, send, address
//! book, sign and verify. Output is line-oriented `key value` text for the
//! regtest suite (`regtest/harness/tests/test_l1_send.py`).

use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Subcommand;
use dw_engine::{
    BookPurpose, ChangePolicy, CoinFilter, CoinSource, Engine, EngineError, FeeMode,
    NetworkSession, Recipient, SendFailure, WalletId,
};
use dw_vault::{Credential, GrantPurpose, LockState};
use zeroize::Zeroizing;

#[derive(Subcommand)]
pub enum PayCommand {
    /// Print a fresh receive address of the wallet (never handed out before).
    Address { wallet: String },
    /// Start SPV and wait until the wallet has scanned `--height`; print
    /// the balances.
    #[command(name = "sync-wallet")]
    SyncWallet {
        wallet: String,
        #[arg(long)]
        height: u32,
        #[arg(long, default_value_t = 180)]
        timeout_secs: u64,
    },
    /// List unspent outputs: `utxo <txid:vout> <amount> conf=<n> locked=<0|1>
    /// reserved=<0|1> change=<0|1>`.
    Utxos {
        wallet: String,
        /// Include user-locked coins.
        #[arg(long)]
        all: bool,
    },
    /// Lock outpoints (`txid:vout`) against coin selection.
    Lock {
        wallet: String,
        outpoints: Vec<String>,
    },
    /// Unlock outpoints.
    Unlock {
        wallet: String,
        outpoints: Vec<String>,
    },
    /// Print locked outpoints.
    Locked { wallet: String },
    /// Pay recipients. `--to address:duffs[:subtract]`, repeatable.
    Send {
        wallet: String,
        #[arg(long = "to", required = true)]
        to: Vec<String>,
        /// Custom fee rate in duffs per kB (default: recommended).
        #[arg(long)]
        fee_per_kb: Option<u64>,
        /// Coin control: spend exactly these outpoints (repeatable).
        #[arg(long = "coin")]
        coins: Vec<String>,
        /// Custom change address.
        #[arg(long)]
        change: Option<String>,
        /// Label saved to the address book for every recipient.
        #[arg(long)]
        label: Option<String>,
        /// Message stored with the transaction.
        #[arg(long)]
        message: Option<String>,
        /// Start SPV and wait for this wallet height before planning.
        #[arg(long)]
        sync_height: Option<u32>,
        /// Prepare (sign and reserve) only; print the raw transaction and
        /// abandon it.
        #[arg(long)]
        no_broadcast: bool,
        /// After a `send.broadcast_unknown` outcome, report the payment's
        /// inputs (`held reserved=<n> unlisted=<n>`), check that abandon is
        /// refused, and broadcast the same prepared transaction again; at
        /// most this many times.
        #[arg(long, default_value_t = 0)]
        rebroadcast_unknown: u32,
    },
    /// Address book: list entries.
    Book { wallet: String },
    /// Address book: add or relabel an entry.
    BookAdd {
        wallet: String,
        address: String,
        label: String,
        #[arg(long)]
        receive: bool,
        #[arg(long)]
        replace: bool,
    },
    /// Sign a message with one of the wallet's P2PKH addresses.
    Sign {
        wallet: String,
        address: String,
        message: String,
    },
    /// Verify a message signature (no wallet needed).
    Verify {
        address: String,
        message: String,
        signature: String,
    },
}

pub(crate) fn wallet_id(s: &str) -> Result<WalletId, String> {
    s.parse().map_err(|e: dw_engine::EngineError| e.to_string())
}

pub(crate) fn outpoint(s: &str) -> Result<dashcore::OutPoint, String> {
    dashcore::OutPoint::from_str(s).map_err(|e| format!("bad outpoint {s:?}: {e}"))
}

/// `address:duffs[:subtract]`.
fn recipient(
    s: &str,
    label: &Option<String>,
    message: &Option<String>,
) -> Result<Recipient, String> {
    let mut parts = s.split(':');
    let (Some(address), Some(amount)) = (parts.next(), parts.next()) else {
        return Err(format!(
            "bad recipient {s:?}; want address:duffs[:subtract]"
        ));
    };
    let subtract = match parts.next() {
        None => false,
        Some("subtract") => true,
        Some(other) => return Err(format!("bad recipient flag {other:?}")),
    };
    Ok(Recipient {
        address: address.to_string(),
        amount: amount
            .parse()
            .map_err(|e| format!("bad amount {amount:?}: {e}"))?,
        subtract_fee_from_amount: subtract,
        label: label.clone(),
        message: message.clone(),
    })
}

/// The credential a grant needs: the passphrase of an encrypted vault, none
/// for an unencrypted one.
pub(crate) fn credential<'a>(
    session: &NetworkSession,
    passphrase: Option<&'a Zeroizing<Vec<u8>>>,
) -> Credential<'a> {
    match (session.vault().status().encrypted, passphrase) {
        (true, Some(p)) => Credential::Passphrase(p),
        _ => Credential::None,
    }
}

pub(crate) fn wait_for_height(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    height: u32,
    timeout: Duration,
) -> Result<(), String> {
    if !session.spv_running().map_err(|e| e.to_string())? {
        engine
            .block_on(session.start_spv())
            .map_err(|e| e.to_string())?;
    }
    let start = Instant::now();
    loop {
        let h = session.wallet_scan_height(&id).unwrap_or(0);
        if h >= height {
            return Ok(());
        }
        if start.elapsed() > timeout {
            return Err(format!(
                "wallet height {h} did not reach {height} in {timeout:?}"
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

pub fn run(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    passphrase: Option<&Zeroizing<Vec<u8>>>,
    cmd: PayCommand,
) -> Result<(), String> {
    let e = |e: dw_engine::EngineError| e.to_string();
    match cmd {
        PayCommand::Address { wallet } => {
            let a = engine
                .block_on(session.next_receive_address(wallet_id(&wallet)?, None))
                .map_err(e)?;
            println!("address {}", a.address);
        }
        PayCommand::SyncWallet {
            wallet,
            height,
            timeout_secs,
        } => {
            let id = wallet_id(&wallet)?;
            wait_for_height(
                engine,
                session,
                id,
                height,
                Duration::from_secs(timeout_secs),
            )?;
            let b = session
                .balances(&id)
                .map_err(e)?
                .ok_or_else(|| format!("balance of {id} still unknown at height {height}"))?;
            println!(
                "synced {height} confirmed={} unconfirmed={} immature={} total={}",
                b.confirmed, b.unconfirmed, b.immature, b.total
            );
        }
        PayCommand::Utxos { wallet, all } => {
            let coins = engine
                .block_on(session.utxos(
                    wallet_id(&wallet)?,
                    CoinFilter {
                        include_locked: all,
                        ..Default::default()
                    },
                ))
                .map_err(e)?;
            for c in coins {
                println!(
                    "utxo {} {} conf={} locked={} reserved={} change={} address={}",
                    c.outpoint,
                    c.amount,
                    c.confirmations,
                    u8::from(c.user_locked),
                    u8::from(c.reserved),
                    u8::from(c.is_change),
                    c.address
                );
            }
        }
        PayCommand::Lock { wallet, outpoints } | PayCommand::Unlock { wallet, outpoints }
            if outpoints.is_empty() =>
        {
            let _ = wallet;
            return Err("no outpoints given".into());
        }
        PayCommand::Lock { wallet, outpoints } => {
            let list = outpoints
                .iter()
                .map(|o| outpoint(o))
                .collect::<Result<_, _>>()?;
            engine
                .block_on(session.lock_outpoints(wallet_id(&wallet)?, list))
                .map_err(e)?;
            println!("locked {}", outpoints.len());
        }
        PayCommand::Unlock { wallet, outpoints } => {
            let list = outpoints
                .iter()
                .map(|o| outpoint(o))
                .collect::<Result<_, _>>()?;
            engine
                .block_on(session.unlock_outpoints(wallet_id(&wallet)?, list))
                .map_err(e)?;
            println!("unlocked {}", outpoints.len());
        }
        PayCommand::Locked { wallet } => {
            for o in engine
                .block_on(session.locked_outpoints(wallet_id(&wallet)?))
                .map_err(e)?
            {
                println!("locked {o}");
            }
        }
        PayCommand::Send {
            wallet,
            to,
            fee_per_kb,
            coins,
            change,
            label,
            message,
            sync_height,
            no_broadcast,
            rebroadcast_unknown,
        } => {
            let id = wallet_id(&wallet)?;
            if let Some(h) = sync_height {
                wait_for_height(engine, session, id, h, Duration::from_secs(180))?;
            }
            let draft = session.new_tx_draft(id).map_err(e)?;
            draft
                .set_recipients(
                    to.iter()
                        .map(|r| recipient(r, &label, &message))
                        .collect::<Result<_, _>>()?,
                )
                .map_err(e)?;
            if let Some(rate) = fee_per_kb {
                draft.set_fee(FeeMode::PerKb(rate)).map_err(e)?;
            }
            if !coins.is_empty() {
                let list = coins
                    .iter()
                    .map(|o| outpoint(o))
                    .collect::<Result<_, _>>()?;
                draft.set_source(CoinSource::Outpoints(list)).map_err(e)?;
            }
            let custom_change = change.is_some();
            if let Some(addr) = change {
                draft.set_change(ChangePolicy::Address(addr)).map_err(e)?;
            }
            let estimate = engine.block_on(draft.estimate()).map_err(e)?;
            println!(
                "estimate fee={} size={} inputs={} change={} sent={}",
                estimate.fee,
                estimate.size_bytes,
                estimate.input_count,
                estimate.change.map_or("none".into(), |c| c.to_string()),
                estimate.total_sent
            );
            // The grant caps what leaves the wallet: the recipients' amounts,
            // plus the change when a custom change address may be foreign
            // (the engine refuses a cap that is too low; never one too high).
            let custom_change_value = if custom_change {
                estimate.change.unwrap_or(0)
            } else {
                0
            };
            let cap = to
                .iter()
                .map(|r| recipient(r, &None, &None).map(|r| r.amount))
                .sum::<Result<u64, _>>()?
                + custom_change_value;
            let grant = session
                .vault()
                .authorize(
                    GrantPurpose::Spend { max_duffs: cap },
                    Some(&id.0),
                    credential(session, passphrase),
                )
                .map_err(|e| e.to_string())?;
            let prepared = engine.block_on(draft.prepare(grant.id)).map_err(e)?;
            let s = prepared.summary();
            println!(
                "prepared txid={} fee={} rate={} size={} sent={} external={} debit={}",
                s.txid,
                s.fee,
                s.fee_rate_per_kb,
                s.size_bytes,
                s.total_sent,
                s.external_sent,
                s.total_debit
            );
            for i in &s.inputs {
                println!("input {} {}", i.outpoint, i.amount);
            }
            for o in &s.outputs {
                println!(
                    "output {} {} change={} mine={}",
                    o.address.as_deref().unwrap_or("-"),
                    o.amount,
                    u8::from(o.is_change),
                    u8::from(o.is_mine)
                );
            }
            if no_broadcast {
                let raw = prepared.raw().map(hex_encode).unwrap_or_default();
                println!("raw {raw}");
                engine.block_on(draft.abandon(prepared)).map_err(e)?;
                println!("abandoned");
            } else {
                let inputs: Vec<dashcore::OutPoint> = s.inputs.iter().map(|i| i.outpoint).collect();
                let mut repeats = 0;
                loop {
                    match engine.block_on(draft.broadcast(Arc::clone(&prepared))) {
                        Ok(outcome) => {
                            println!("broadcast {}", outcome.txid);
                            break;
                        }
                        Err(EngineError::Send(SendFailure::BroadcastUnknown { reason }))
                            if repeats < rebroadcast_unknown =>
                        {
                            repeats += 1;
                            println!("unknown {} reason={}", s.txid, reason.replace(' ', "_"));
                            // The inputs are either still reserved or already
                            // seen spent by the transaction; never selectable.
                            let coins = engine
                                .block_on(session.utxos(
                                    id,
                                    CoinFilter {
                                        include_locked: true,
                                        ..Default::default()
                                    },
                                ))
                                .map_err(e)?;
                            let reserved = coins
                                .iter()
                                .filter(|c| inputs.contains(&c.outpoint) && c.reserved)
                                .count();
                            let unlisted = inputs
                                .iter()
                                .filter(|o| !coins.iter().any(|c| c.outpoint == **o))
                                .count();
                            println!("held reserved={reserved} unlisted={unlisted}");
                            match engine.block_on(draft.abandon(Arc::clone(&prepared))) {
                                Err(EngineError::Send(SendFailure::PreparedTxSpent)) => {
                                    println!("abandon refused");
                                }
                                other => {
                                    return Err(format!(
                                        "abandon after an unknown outcome: {other:?}"
                                    ));
                                }
                            }
                            println!("rebroadcast {repeats}");
                        }
                        Err(err) => return Err(err.to_string()),
                    }
                }
            }
        }
        PayCommand::Book { wallet } => {
            for entry in engine
                .block_on(session.address_book(wallet_id(&wallet)?, None, None))
                .map_err(e)?
            {
                println!("book {:?} {} {}", entry.purpose, entry.address, entry.label);
            }
        }
        PayCommand::BookAdd {
            wallet,
            address,
            label,
            receive,
            replace,
        } => {
            let purpose = if receive {
                BookPurpose::Receive
            } else {
                BookPurpose::Send
            };
            let entry = engine
                .block_on(session.save_address_book_entry(
                    wallet_id(&wallet)?,
                    address,
                    label,
                    purpose,
                    replace,
                ))
                .map_err(e)?;
            println!("book {:?} {} {}", entry.purpose, entry.address, entry.label);
        }
        PayCommand::Sign {
            wallet,
            address,
            message,
        } => {
            if session.vault().lock_state() == LockState::Locked && passphrase.is_none() {
                return Err("vault locked; pass --passphrase-file".into());
            }
            let id = wallet_id(&wallet)?;
            let grant = session
                .vault()
                .authorize(
                    GrantPurpose::SignMessage,
                    Some(&id.0),
                    credential(session, passphrase),
                )
                .map_err(|e| e.to_string())?;
            let sig = engine
                .block_on(session.sign_message(id, address, message.into_bytes(), grant.id))
                .map_err(e)?;
            println!("signature {sig}");
        }
        PayCommand::Verify {
            address,
            message,
            signature,
        } => {
            let network = session.network().core_network();
            match dw_message::verify_message(&address, &signature, message.as_bytes(), network) {
                Ok(()) => println!("verified"),
                Err(err) => return Err(format!("not verified: {err:?}")),
            }
        }
    }
    Ok(())
}

fn hex_encode(bytes: Vec<u8>) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
