//! M3 R2 governance commands: govsync, the proposal list and info panel,
//! creating / resuming / submitting proposals, voting, and the G7 govsync
//! measurement. Output is line-oriented `key=value` text for the regtest
//! `governance` suite (`regtest/functional/dwd_governance.py`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Subcommand;
use dw_engine::governance::{
    GovernanceSyncPhase, ProposalDraft, ProposalQuery, ProposalSource, VoteOutcome,
};
use dw_engine::{Engine, NetworkSession, WalletId};
use dw_vault::GrantPurpose;
use zeroize::Zeroizing;

use crate::pay::credential;

#[derive(Subcommand)]
pub enum GovCommand {
    /// The first wallet's DIP-3 voting key addresses: `voting_address <addr>
    /// path=<path>`, `--count` of them.
    VotingAddress {
        #[arg(long, default_value_t = 1)]
        count: usize,
    },
    /// Sync SPV and governance, then print the proposal list
    /// (`proposal hash=… status=… yes=… no=… abstain=… margin=… my=…`).
    List {
        #[command(flatten)]
        wait: Wait,
        /// "My Proposals" of the first wallet instead of the active ones.
        #[arg(long)]
        mine: bool,
        #[arg(long)]
        title: Option<String>,
        /// Keep waiting until this proposal has at least `--min-yes`
        /// weighted yes votes.
        #[arg(long)]
        wait_for: Option<String>,
        #[arg(long, default_value_t = 0)]
        min_yes: u32,
    },
    /// Sync, then print the info panel (`info key=value…`) and the clock.
    Info {
        #[command(flatten)]
        wait: Wait,
    },
    /// Create a proposal (`gobject prepare`): pays the 1 DASH collateral.
    /// Prints `proposal <hash> collateral=<txid>`.
    Prepare {
        #[command(flatten)]
        wait: Wait,
        #[arg(long)]
        name: String,
        #[arg(long)]
        url: String,
        #[arg(long)]
        address: String,
        /// Per payment, duffs.
        #[arg(long)]
        amount: u64,
        #[arg(long, default_value_t = 1)]
        count: u32,
        /// First payment superblock; the next one when omitted.
        #[arg(long)]
        first_superblock: Option<u32>,
        /// Keep SPV running this long after the broadcast.
        #[arg(long, default_value_t = 5)]
        linger_secs: u64,
    },
    /// The first wallet's pending proposals (`pending <hash> status=…
    /// confirmations=…`).
    Pending {
        #[command(flatten)]
        wait: Wait,
    },
    /// `gobject submit` of a pending proposal.
    Submit {
        #[command(flatten)]
        wait: Wait,
        hash: String,
    },
    /// Vote with every masternode whose voting key the first wallet holds
    /// (or `--masternode`), after governance synced.
    Vote {
        #[command(flatten)]
        wait: Wait,
        hash: String,
        /// yes | no | abstain
        outcome: String,
        #[arg(long = "masternode")]
        masternodes: Vec<String>,
    },
    /// G7: govsync from this data dir, logging per phase (objects, votes,
    /// bytes, wall time, peak RSS) as `measure key=value…`.
    Sync {
        #[arg(long, default_value_t = 1800)]
        timeout_secs: u64,
    },
}

/// When to start: SPV synced to a height, governance sync enabled.
#[derive(clap::Args, Clone)]
pub struct Wait {
    /// Wait until the best header is at least this height.
    #[arg(long)]
    sync_height: Option<u32>,
    #[arg(long, default_value_t = 300)]
    timeout_secs: u64,
    /// Turn governance sync on and wait until it synced.
    #[arg(long)]
    gov: bool,
}

fn outcome(s: &str) -> Result<VoteOutcome, String> {
    match s {
        "yes" => Ok(VoteOutcome::Yes),
        "no" => Ok(VoteOutcome::No),
        "abstain" => Ok(VoteOutcome::Abstain),
        other => Err(format!("unknown outcome {other:?}")),
    }
}

fn first_wallet(session: &Arc<NetworkSession>) -> Result<WalletId, String> {
    session
        .wallet_infos()
        .map_err(|e| e.to_string())?
        .first()
        .map(|w| w.wallet_id)
        .ok_or_else(|| "no wallet on this network".into())
}

/// Starts SPV, waits for the height, then (with `--gov`) for govsync.
fn prepare_session(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    wait: &Wait,
) -> Result<Instant, String> {
    let deadline = Instant::now() + Duration::from_secs(wait.timeout_secs);
    if !session.spv_running().map_err(|e| e.to_string())? {
        engine
            .block_on(session.start_spv())
            .map_err(|e| e.to_string())?;
    }
    if let Some(h) = wait.sync_height {
        loop {
            let tip = session
                .sync_snapshot()
                .map_err(|e| e.to_string())?
                .tip_height
                .unwrap_or(0);
            // Wallet scans follow headers; wait for both.
            let scanned = session
                .wallet_infos()
                .map_err(|e| e.to_string())?
                .iter()
                .all(|w| session.wallet_scan_height(&w.wallet_id).is_some_and(|s| s >= h));
            if tip >= h && scanned {
                break;
            }
            if Instant::now() > deadline {
                return Err(format!("height {h} not reached (tip {tip})"));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    if wait.gov {
        wait_gov_synced(engine, session, deadline)?;
    }
    Ok(deadline)
}

fn wait_gov_synced(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    deadline: Instant,
) -> Result<(), String> {
    engine
        .block_on(session.set_governance_sync_enabled(true))
        .map_err(|e| e.to_string())?;
    let mut last = String::new();
    loop {
        let s = session.governance_sync_state().map_err(|e| e.to_string())?;
        let line = format!(
            "govsync phase={:?} objects={} votes={} peers={} bytes={} error={}",
            s.phase,
            s.objects,
            s.votes,
            s.peers,
            s.bytes_received,
            s.last_error.as_deref().unwrap_or("-")
        );
        if line != last {
            eprintln!("{line}");
            last = line;
        }
        if s.phase == GovernanceSyncPhase::Synced {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(format!("governance did not sync: {last}"));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Resident set size of this process, bytes (`ps`); govsync only grows its
/// store, so at the end this is the peak.
fn rss() -> Option<u64> {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<u64>()
        .ok()
        .map(|kb| kb * 1024)
}

pub fn run(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    passphrase: Option<&Zeroizing<Vec<u8>>>,
    cmd: GovCommand,
) -> Result<(), String> {
    let e = |e: dw_engine::EngineError| e.to_string();
    match cmd {
        GovCommand::VotingAddress { count } => {
            let id = first_wallet(session)?;
            let addrs = engine
                .block_on(session.governance_voting_addresses(id))
                .map_err(e)?;
            for (addr, path) in addrs.into_iter().take(count) {
                println!("voting_address {addr} path={path}");
            }
        }
        GovCommand::List {
            wait,
            mine,
            title,
            wait_for,
            min_yes,
        } => {
            let deadline = prepare_session(engine, session, &wait)?;
            let source = if mine {
                ProposalSource::Mine(first_wallet(session)?)
            } else {
                ProposalSource::Active
            };
            let query = ProposalQuery {
                source,
                title_filter: title,
            };
            let rows = loop {
                let rows = engine
                    .block_on(session.proposals(query.clone()))
                    .map_err(e)?;
                let done = match &wait_for {
                    None => true,
                    Some(h) => rows.iter().any(|r| &r.hash == h && r.yes >= min_yes),
                };
                if done {
                    break rows;
                }
                if Instant::now() > deadline {
                    return Err(format!("proposal {wait_for:?} with {min_yes} yes not seen"));
                }
                std::thread::sleep(Duration::from_millis(500));
            };
            for r in rows {
                let my = r.my_votes.map_or("none".to_string(), |m| {
                    format!("{}/{}/{}/{}", m.yes, m.no, m.abstain, m.unvoted)
                });
                println!(
                    "proposal hash={} name={} status={:?} yes={} no={} abstain={} margin={} my={} amount={} confirmations={}",
                    r.hash,
                    r.name,
                    r.status,
                    r.yes,
                    r.no,
                    r.abstain,
                    r.margin,
                    my,
                    r.payment_amount,
                    r.collateral_confirmations
                        .map_or("-".to_string(), |c| c.to_string())
                );
            }
        }
        GovCommand::Info { wait } => {
            prepare_session(engine, session, &wait)?;
            let i = engine.block_on(session.governance_info()).map_err(e)?;
            let o = |v: Option<u32>| v.map_or("-".to_string(), |x| x.to_string());
            let a = |v: Option<u64>| v.map_or("-".to_string(), |x| x.to_string());
            println!(
                "info cycle={} last={} next={} cutoff={} threshold={} mn_eligible={} evo_eligible={} mn_voting={} controlled={} votes_controlled={} proposals={} passing={} failing={} unfunded={} budget={} allocated={}",
                i.superblock_cycle,
                o(i.last_superblock),
                o(i.next_superblock),
                o(i.voting_cutoff),
                o(i.passing_threshold),
                o(i.masternodes_eligible),
                o(i.evonodes_eligible),
                o(i.masternodes_voting),
                i.masternodes_controlled,
                i.votes_controlled,
                o(i.proposal_count),
                o(i.passing),
                o(i.failing),
                o(i.unfunded),
                a(i.budget_available),
                a(i.budget_allocated),
            );
            let c = session.governance_clock().map_err(e)?;
            println!(
                "clock next={} blocks={} cutoff={} open={} progress={:.3} committed={}",
                c.next_superblock,
                c.blocks_to_superblock,
                c.voting_cutoff,
                c.voting_open,
                c.cycle_progress,
                c.budget_committed
                    .map_or("-".to_string(), |v| format!("{v:.3}"))
            );
        }
        GovCommand::Prepare {
            wait,
            name,
            url,
            address,
            amount,
            count,
            first_superblock,
            linger_secs,
        } => {
            prepare_session(engine, session, &wait)?;
            let id = first_wallet(session)?;
            let first = match first_superblock {
                Some(h) => h,
                None => {
                    session
                        .superblock_dates(1)
                        .map_err(e)?
                        .first()
                        .ok_or("no superblock date")?
                        .height
                }
            };
            let draft = ProposalDraft {
                name,
                url,
                payment_address: address,
                payment_amount: amount,
                payment_count: count,
                first_superblock_height: first,
            };
            println!("json {}", session.proposal_json(&draft).map_err(e)?);
            let grant = session
                .vault()
                .authorize(
                    GrantPurpose::Spend {
                        max_duffs: 100_000_000 + 10_000_000,
                    },
                    Some(&id.0),
                    credential(session, passphrase),
                )
                .map_err(|e| e.to_string())?;
            let p = engine
                .block_on(session.create_proposal(id, draft, grant.id))
                .map_err(e)?;
            println!("proposal {} collateral={}", p.hash, p.collateral_txid);
            std::thread::sleep(Duration::from_secs(linger_secs));
        }
        GovCommand::Pending { wait } => {
            prepare_session(engine, session, &wait)?;
            let id = first_wallet(session)?;
            for p in engine.block_on(session.pending_proposals(id)).map_err(e)? {
                println!(
                    "pending {} name={} status={:?} confirmations={} collateral={} payments={}",
                    p.hash, p.name, p.collateral_status, p.confirmations, p.collateral_txid, p.payment_count
                );
            }
        }
        GovCommand::Submit { wait, hash } => {
            prepare_session(engine, session, &wait)?;
            let id = first_wallet(session)?;
            let h = engine
                .block_on(session.submit_proposal(id, hash))
                .map_err(e)?;
            println!("submitted {h}");
        }
        GovCommand::Vote {
            wait,
            hash,
            outcome: o,
            masternodes,
        } => {
            let deadline = prepare_session(engine, session, &wait)?;
            let id = first_wallet(session)?;
            let outcome = outcome(&o)?;
            let mns = loop {
                match engine.block_on(session.voting_masternodes(hash.clone(), Some(id))) {
                    Ok(m) if !m.is_empty() => break m,
                    Ok(_) | Err(_) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(500));
                    }
                    Ok(_) => return Err("no voting masternodes".into()),
                    Err(err) => return Err(err.to_string()),
                }
            };
            for m in &mns {
                println!(
                    "voting_masternode {} collateral={} address={} weight={} current={:?}",
                    m.pro_tx_hash, m.collateral, m.voting_address, m.weight, m.current_vote
                );
            }
            let chosen: Vec<String> = if masternodes.is_empty() {
                mns.iter().map(|m| m.pro_tx_hash.clone()).collect()
            } else {
                masternodes
            };
            let grant = session
                .vault()
                .authorize(GrantPurpose::Governance, Some(&id.0), credential(session, passphrase))
                .map_err(|e| e.to_string())?;
            let results = engine
                .block_on(session.cast_votes(hash, outcome, chosen, grant.id))
                .map_err(e)?;
            for r in results {
                match r.failure {
                    None => println!("vote {} ok {}", r.pro_tx_hash, r.detail.unwrap_or_default()),
                    Some(f) => println!("vote {} failed {f}", r.pro_tx_hash),
                }
            }
        }
        GovCommand::Sync { timeout_secs } => {
            let started = Instant::now();
            let deadline = started + Duration::from_secs(timeout_secs);
            if !session.spv_running().map_err(|e| e.to_string())? {
                engine.block_on(session.start_spv()).map_err(e)?;
            }
            // The masternode list must sync first (peers and vote checks).
            loop {
                let i = engine.block_on(session.governance_info()).map_err(e)?;
                if i.masternodes_eligible.is_some() {
                    break;
                }
                if Instant::now() > deadline {
                    return Err("the masternode list did not sync".into());
                }
                std::thread::sleep(Duration::from_secs(1));
            }
            let spv_secs = started.elapsed().as_secs_f64();
            let gov_started = Instant::now();
            wait_gov_synced(engine, session, deadline)?;
            let s = session.governance_sync_state().map_err(e)?;
            println!(
                "measure spv_secs={spv_secs:.1} gov_secs={:.1} objects_secs={} votes_secs={} objects={} votes={} peers={} bytes={} rss={}",
                gov_started.elapsed().as_secs_f64(),
                s.objects_secs.map_or("-".into(), |v| format!("{v:.1}")),
                s.votes_secs.map_or("-".into(), |v| format!("{v:.1}")),
                s.objects,
                s.votes,
                s.peers,
                s.bytes_received,
                rss().map_or("-".into(), |v| v.to_string()),
            );
            let rows = engine
                .block_on(session.proposals(ProposalQuery {
                    source: ProposalSource::Active,
                    title_filter: None,
                }))
                .map_err(e)?;
            for r in rows {
                println!(
                    "tally hash={} yes={} no={} abstain={} status={:?}",
                    r.hash, r.yes, r.no, r.abstain, r.status
                );
            }
        }
    }
    Ok(())
}
