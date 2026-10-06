//! M3 R1 commands: CoinJoin options, status, mixing, coins, salt, recovery
//! and "move mixed coins". Output is line-oriented `key=value` text for the
//! regtest suite (`regtest/functional/dwd_coinjoin_client.py`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Subcommand;
use dw_engine::coinjoin::{CoinJoinSettings, CoinJoinStatus, MixingState, SweepDestination};
use dw_engine::{CoinFilter, Engine, NetworkSession, WalletId};
use dw_vault::{GrantPurpose, LockState, UnlockScope};
use zeroize::Zeroizing;

use crate::pay::{credential, wait_for_height, wallet_id};

#[derive(Subcommand)]
pub enum CoinJoinCommand {
    /// Print the CoinJoin options; `--set key=value` (repeatable) changes
    /// them first: enabled, multi, sessions, rounds, amount, goal, cap.
    #[command(name = "coinjoin-settings")]
    Settings {
        #[arg(long = "set")]
        set: Vec<String>,
    },
    /// Print one wallet's CoinJoin status line.
    #[command(name = "coinjoin-status")]
    Status { wallet: String },
    /// Start SPV and mixing, print a status line on every change, and stop
    /// mixing when a condition holds or the time runs out (exit status 1).
    #[command(name = "coinjoin-mix")]
    Mix {
        wallet: String,
        #[arg(long, default_value_t = 900)]
        timeout_secs: u64,
        /// Wait for this wallet height before starting.
        #[arg(long)]
        sync_height: Option<u32>,
        /// Done once a denomination has at least this many rounds.
        #[arg(long)]
        until_rounds: Option<u32>,
        /// Done once the fully mixed balance reaches this many duffs.
        #[arg(long)]
        until_fully_mixed: Option<u64>,
        /// Done as soon as a session holds coins (to test stop).
        #[arg(long)]
        until_session_entry: bool,
        /// Unlock an encrypted vault for mixing only (QT-112).
        #[arg(long)]
        mixing_only: bool,
    },
    /// Coins with their mixing rounds: `cjutxo <txid:vout> <amount>
    /// rounds=<n|na> denom=<0|1> reserved=<0|1> conf=<n>`.
    #[command(name = "coinjoin-utxos")]
    Utxos {
        wallet: String,
        /// Only fully mixed coins (the CoinJoin send page's).
        #[arg(long)]
        fully_mixed: bool,
    },
    /// `coinjoinsalt get`, or `--set HEX` / `--generate`.
    #[command(name = "coinjoin-salt")]
    Salt {
        wallet: String,
        #[arg(long)]
        set: Option<String>,
        #[arg(long)]
        generate: bool,
    },
    /// IOS-057 recovery scan (starts SPV).
    #[command(name = "coinjoin-recover")]
    Recover {
        wallet: String,
        #[arg(long)]
        sync_height: Option<u32>,
    },
    /// IOS-057 "move mixed coins" to a fresh address of the wallet; with
    /// `--plan-only` print the chunks only.
    #[command(name = "move-mixed")]
    MoveMixed {
        wallet: String,
        #[arg(long)]
        plan_only: bool,
        #[arg(long)]
        sync_height: Option<u32>,
    },
}

impl CoinJoinCommand {
    /// Whether the command mixes and wants a mixing-only unlock.
    pub fn mixing_only(&self) -> bool {
        matches!(self, Self::Mix { mixing_only: true, .. })
    }
}

fn status_line(s: &CoinJoinStatus) -> String {
    format!(
        "cjstatus state={:?} status={:?} sessions={} queue={} progress={:.2} anonymizable={} denominated={} normalized={} fully_mixed={} avg_rounds={:.2} unavailable={:?} stop_reason={:?} denoms={:?}",
        s.state,
        s.status,
        s.sessions.len(),
        s.queue_size,
        s.progress.overall_percent,
        s.balances.anonymizable,
        s.balances.denominated,
        s.balances.normalized_anonymized,
        s.balances.fully_mixed,
        s.progress.average_rounds,
        s.unavailable,
        s.stop_reason,
        s.submitted_denominations
    )
}

fn apply_setting(s: &mut CoinJoinSettings, kv: &str) -> Result<(), String> {
    let (k, v) = kv
        .split_once('=')
        .ok_or_else(|| format!("bad setting {kv:?}; want key=value"))?;
    let n = || v.parse::<u32>().map_err(|e| format!("{k}: {e}"));
    let b = || match v {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        _ => Err(format!("{k}: want 0 or 1")),
    };
    match k {
        "enabled" => s.enabled = b()?,
        "multi" => s.multi_session = b()?,
        "sessions" => s.max_sessions = n()?,
        "rounds" => s.rounds = n()?,
        "amount" => s.target_amount_dash = n()?,
        "goal" => s.denoms_goal = n()?,
        "cap" => s.denoms_hard_cap = n()?,
        other => return Err(format!("unknown setting {other:?}")),
    }
    Ok(())
}

/// Highest rounds of any denomination, and the reserved coin count.
fn coin_facts(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
) -> Result<(u32, usize), String> {
    let coins = engine
        .block_on(session.utxos(
            id,
            CoinFilter {
                include_locked: true,
                ..Default::default()
            },
        ))
        .map_err(|e| e.to_string())?;
    let max_rounds = coins
        .iter()
        .filter_map(|c| c.coinjoin_rounds)
        .max()
        .unwrap_or(0);
    let reserved = coins.iter().filter(|c| c.reserved).count();
    Ok((max_rounds, reserved))
}

pub fn run(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    passphrase: Option<&Zeroizing<Vec<u8>>>,
    cmd: CoinJoinCommand,
) -> Result<(), String> {
    let e = |e: dw_engine::EngineError| e.to_string();
    match cmd {
        CoinJoinCommand::Settings { set } => {
            let mut s = session.coinjoin_settings().map_err(e)?;
            if !set.is_empty() {
                for kv in &set {
                    apply_setting(&mut s, kv)?;
                }
                engine.block_on(session.set_coinjoin_settings(s)).map_err(e)?;
            }
            let s = session.coinjoin_settings().map_err(e)?;
            println!(
                "cjsettings enabled={} multi={} sessions={} rounds={} amount={} goal={} cap={}",
                u8::from(s.enabled),
                u8::from(s.multi_session),
                s.max_sessions,
                s.rounds,
                s.target_amount_dash,
                s.denoms_goal,
                s.denoms_hard_cap
            );
        }
        CoinJoinCommand::Status { wallet } => {
            let s = session.coinjoin_status(wallet_id(&wallet)?).map_err(e)?;
            println!("{}", status_line(&s));
        }
        CoinJoinCommand::Mix {
            wallet,
            timeout_secs,
            sync_height,
            until_rounds,
            until_fully_mixed,
            until_session_entry,
            mixing_only,
        } => {
            let id = wallet_id(&wallet)?;
            if mixing_only
                && let Some(p) = passphrase
                && session.vault().lock_state() == LockState::Locked
            {
                let p = p.clone();
                engine
                    .block_on(session.vault_op(move |v| v.unlock(&p, UnlockScope::MixingOnly)))
                    .map_err(e)?;
            }
            println!("vault {:?}", session.vault().lock_state());
            wait_for_height(
                engine,
                session,
                id,
                sync_height.unwrap_or(0),
                Duration::from_secs(300),
            )?;
            engine.block_on(session.start_mixing(id)).map_err(e)?;
            let deadline = Instant::now() + Duration::from_secs(timeout_secs);
            let mut last = String::new();
            let mut last_facts = (u32::MAX, usize::MAX);
            let outcome = loop {
                let s = session.coinjoin_status(id).map_err(e)?;
                let line = status_line(&s);
                if line != last {
                    println!("{line}");
                    last = line;
                }
                let facts = coin_facts(engine, session, id)?;
                if facts != last_facts {
                    println!("cjcoins max_rounds={} reserved={}", facts.0, facts.1);
                    last_facts = facts;
                }
                if s.state != MixingState::Mixing {
                    break Err(format!("mixing stopped: {:?}", s.stop_reason));
                }
                let entered = s
                    .sessions
                    .iter()
                    .any(|x| x.entries > 0);
                let done = until_rounds.is_some_and(|r| facts.0 >= r)
                    || until_fully_mixed.is_some_and(|d| s.balances.fully_mixed >= d)
                    || (until_session_entry && entered);
                if done {
                    break Ok(());
                }
                if Instant::now() >= deadline {
                    break Err(format!("condition not reached within {timeout_secs}s"));
                }
                std::thread::sleep(Duration::from_millis(500));
            };
            engine.block_on(session.stop_mixing(id)).map_err(e)?;
            let s = session.coinjoin_status(id).map_err(e)?;
            println!("stopped {}", status_line(&s));
            let (max_rounds, reserved) = coin_facts(engine, session, id)?;
            println!("after_stop max_rounds={max_rounds} reserved={reserved}");
            outcome?;
            println!("mixed max_rounds={max_rounds} fully_mixed={}", s.balances.fully_mixed);
        }
        CoinJoinCommand::Utxos { wallet, fully_mixed } => {
            let coins = engine
                .block_on(session.utxos(
                    wallet_id(&wallet)?,
                    CoinFilter {
                        include_locked: true,
                        fully_mixed_only: fully_mixed,
                        min_confirmations: None,
                    },
                ))
                .map_err(e)?;
            for c in coins {
                println!(
                    "cjutxo {} {} rounds={} denom={} reserved={} conf={}",
                    c.outpoint,
                    c.amount,
                    c.coinjoin_rounds.map_or("na".into(), |r| r.to_string()),
                    u8::from(c.coinjoin_denominated),
                    u8::from(c.reserved),
                    c.confirmations
                );
            }
        }
        CoinJoinCommand::Salt {
            wallet,
            set,
            generate,
        } => {
            let id = wallet_id(&wallet)?;
            if let Some(hex) = set {
                engine
                    .block_on(session.set_coinjoin_salt(id, hex))
                    .map_err(e)?;
            } else if generate {
                engine
                    .block_on(session.generate_coinjoin_salt(id))
                    .map_err(e)?;
            }
            let salt = engine.block_on(session.coinjoin_salt(id)).map_err(e)?;
            println!("salt {salt}");
        }
        CoinJoinCommand::Recover {
            wallet,
            sync_height,
        } => {
            let id = wallet_id(&wallet)?;
            wait_for_height(
                engine,
                session,
                id,
                sync_height.unwrap_or(0),
                Duration::from_secs(300),
            )?;
            let r = engine
                .block_on(session.coinjoin_recovery_scan(id))
                .map_err(e)?;
            println!(
                "recovered coinjoin_scanned={} bip44_scanned={} coinjoin_balance={} new_transactions={}",
                r.coinjoin_addresses_scanned,
                r.bip44_addresses_scanned,
                r.coinjoin_balance,
                r.new_transactions
            );
        }
        CoinJoinCommand::MoveMixed {
            wallet,
            plan_only,
            sync_height,
        } => {
            let id = wallet_id(&wallet)?;
            if let Some(h) = sync_height {
                wait_for_height(engine, session, id, h, Duration::from_secs(300))?;
            }
            let plan = engine
                .block_on(session.mixed_coins_sweep_plan(id, SweepDestination::Wallet))
                .map_err(e)?;
            println!("plan total={} chunks={}", plan.total, plan.chunks.len());
            for c in &plan.chunks {
                println!("chunk inputs={} amount={} fee={}", c.inputs, c.amount, c.fee);
            }
            if !plan_only {
                let grant = session
                    .vault()
                    .authorize(
                        GrantPurpose::Spend {
                            max_duffs: plan.total,
                        },
                        Some(&id.0),
                        credential(session, passphrase),
                    )
                    .map_err(|e| e.to_string())?;
                let r = engine
                    .block_on(session.move_mixed_coins(id, SweepDestination::Wallet, grant.id))
                    .map_err(e)?;
                println!(
                    "moved {} remaining={} txids={} failure={}",
                    r.moved,
                    r.remaining,
                    r.txids.join(","),
                    r.failure_code.as_deref().unwrap_or("-")
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_parse() {
        let mut s = CoinJoinSettings::default();
        apply_setting(&mut s, "rounds=2").unwrap();
        apply_setting(&mut s, "multi=1").unwrap();
        assert_eq!((s.rounds, s.multi_session), (2, true));
        assert!(apply_setting(&mut s, "multi=yes").is_err());
        assert!(apply_setting(&mut s, "moon=1").is_err());
        assert!(apply_setting(&mut s, "rounds").is_err());
    }
}
