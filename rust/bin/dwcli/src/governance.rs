//! MG-02 command: governance sync through the facade, for the regtest suite
//! (`regtest/functional/dwd_governance.py`) and the G7 mainnet measurement;
//! with `--propose`, MG-06's proposal creation (MG-06 D17).
//! Output is one record per line, `<kind> <json>`, so the callers parse the
//! facade's records as they serialize.

use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Args;
use dw_console::{ConsoleContext, ConsoleFailure};
use dw_engine::governance::{
    GovernanceSyncPhase, MyProposal, ProposalDraft, ProposalQuery, ProposalScope, RawVote,
    VoteOutcome,
};
use dw_engine::{Engine, NetworkSession, WalletId};
use dw_vault::GrantPurpose;
use serde::Serialize;
use zeroize::Zeroizing;

use crate::pay::{credential, wallet_id};

#[derive(Args)]
pub struct GovernanceArgs {
    /// Give up after this many seconds (exit status 1).
    #[arg(long, default_value_t = 600)]
    timeout_secs: u64,
    /// Wait for this SPV tip before turning governance sync on.
    #[arg(long)]
    sync_height: Option<u32>,
    /// Before turning sync on, wait for `info` to count the list's
    /// masternodes and print it (`offinfo`): reads follow the list with sync
    /// off.
    #[arg(long)]
    offline_info: bool,
    /// Done once synced with at least this many proposals listed.
    #[arg(long, default_value_t = 0)]
    min_proposals: usize,
    /// Done once the proposals' voters add up to at least this many.
    #[arg(long, default_value_t = 0)]
    min_voters: u32,
    /// Done once the proposals' ranged (unresolved) votes add up to at most
    /// this many: with `--collateral-rpc`, once every voter is resolved
    /// (MG-02 D80).
    #[arg(long)]
    max_ranged: Option<u32>,
    /// Print the current votes of this proposal; repeatable.
    #[arg(long = "votes")]
    votes: Vec<String>,
    /// Relay a signed funding vote once synced:
    /// `<txid>-<n>:<proposal>:<yes|no|abstain>:<time>:<base64 signature>`.
    #[arg(long)]
    relay: Option<String>,
    /// Run this console line once synced; repeatable.
    #[arg(long = "console")]
    console: Vec<String>,
    /// Stay connected this long at the end (peers fetch relayed votes).
    #[arg(long, default_value_t = 0)]
    linger_secs: u64,
    /// The wallet of `--propose` and `--wait-mine`; its own proposals are
    /// printed (`mine`) at the end.
    #[arg(long)]
    wallet: Option<String>,
    /// Create a proposal before turning sync on, paying its fee under a
    /// Spend grant of exactly the fee plus the network fee:
    /// `name:url:address:amount-duffs:payment-count:first-superblock`.
    #[arg(long)]
    propose: Option<String>,
    /// Once synced, wait until every own proposal of `--wallet` is in this
    /// state (`confirming`, `public`, ...).
    #[arg(long)]
    wait_mine: Option<String>,
}

impl GovernanceArgs {
    /// Whether the run creates a proposal (it needs the vault unlocked).
    pub fn proposes(&self) -> bool {
        self.propose.is_some()
    }
}

fn line<T: Serialize>(kind: &str, value: &T) {
    println!(
        "{kind} {}",
        serde_json::to_string(value).unwrap_or_else(|e| format!("\"<{e}>\""))
    );
}

/// `name:url:address:amount-duffs:payment-count:first-superblock`; the URL
/// may hold colons.
fn parse_propose(text: &str) -> Result<ProposalDraft, String> {
    let bad =
        || format!("--propose needs name:url:address:amount:count:first-superblock: {text:?}");
    let (name, rest) = text.split_once(':').ok_or_else(bad)?;
    let mut tail: Vec<&str> = rest.rsplitn(5, ':').collect();
    tail.reverse();
    let [url, address, amount, count, first] = tail[..] else {
        return Err(bad());
    };
    Ok(ProposalDraft {
        name: name.into(),
        url: url.into(),
        payment_address: address.into(),
        payment_amount: amount
            .parse()
            .map_err(|e| format!("bad amount {amount:?}: {e}"))?,
        payment_count: count
            .parse()
            .map_err(|e| format!("bad count {count:?}: {e}"))?,
        first_superblock_height: first
            .parse()
            .map_err(|e| format!("bad superblock {first:?}: {e}"))?,
        coins: Vec::new(),
    })
}

/// The `kind` of a proposal's state, as it serializes.
fn state_kind(p: &MyProposal) -> String {
    serde_json::to_value(p.state)
        .ok()
        .and_then(|v| v.get("kind")?.as_str().map(String::from))
        .unwrap_or_default()
}

fn parse_relay(text: &str) -> Result<RawVote, String> {
    let parts: Vec<&str> = text.splitn(5, ':').collect();
    let [collateral, proposal, outcome, time, signature] = parts[..] else {
        return Err(format!("--relay needs 5 fields: {text:?}"));
    };
    let outcome = match outcome {
        "yes" => VoteOutcome::Yes,
        "no" => VoteOutcome::No,
        "abstain" => VoteOutcome::Abstain,
        o => return Err(format!("bad outcome {o:?}")),
    };
    Ok(RawVote {
        collateral: collateral.into(),
        proposal: proposal.into(),
        outcome,
        time: time
            .parse()
            .map_err(|e| format!("bad time {time:?}: {e}"))?,
        signature: signature.into(),
    })
}

pub fn run(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    passphrase: Option<&Zeroizing<Vec<u8>>>,
    args: GovernanceArgs,
) -> Result<(), String> {
    let e = |err: dw_engine::governance::GovernanceError| format!("{} {err}", err.code());
    let timeout = Duration::from_secs(args.timeout_secs);
    let relay = args.relay.as_deref().map(parse_relay).transpose()?;
    let propose = args.propose.as_deref().map(parse_propose).transpose()?;
    let wallet: Option<WalletId> = args.wallet.as_deref().map(wallet_id).transpose()?;
    if (propose.is_some() || args.wait_mine.is_some()) && wallet.is_none() {
        return Err("--propose and --wait-mine need --wallet".into());
    }
    let start = Instant::now();
    crate::pay::ensure_spv_running(engine, session, timeout)?;
    if let Some(height) = args.sync_height {
        loop {
            let tip = session
                .sync_snapshot()
                .map_err(|e| e.to_string())?
                .tip_height;
            if tip.is_some_and(|t| t >= height) {
                break;
            }
            if start.elapsed() > timeout {
                return Err(format!("tip {tip:?} did not reach {height}"));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    let gov = session.governance();
    let all_proposals = || {
        gov.proposals(ProposalQuery {
            scope: ProposalScope::All,
            text: None,
        })
    };
    if args.offline_info {
        loop {
            let info = gov.info().map_err(e)?;
            if info.valid_masternodes > 0 {
                line("offinfo", &info);
                break;
            }
            if start.elapsed() > timeout {
                return Err("no masternode list with sync off".into());
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    if let (Some(draft), Some(id)) = (propose, wallet) {
        let preview = engine
            .block_on(gov.preview_proposal(id, draft.clone()))
            .map_err(e)?;
        line("preview", &preview);
        let grant = session
            .vault()
            .authorize(
                GrantPurpose::Spend {
                    max_duffs: preview.fee + preview.network_fee,
                },
                Some(&id.0),
                credential(session, passphrase),
            )
            .map_err(|e| e.to_string())?;
        let created = engine
            .block_on(gov.create_proposal(id, draft, grant.id))
            .map_err(e)?;
        line("created", &created);
    }
    engine.block_on(gov.set_sync_enabled(true)).map_err(e)?;

    // Synced, with the proposals and voters asked for.
    let mut last_phase = None;
    loop {
        let state = gov.sync_state().map_err(e)?;
        if last_phase != Some(state.phase) {
            last_phase = Some(state.phase);
            line("govstate", &state);
        }
        if state.phase == GovernanceSyncPhase::Synced {
            let rows = all_proposals().map_err(e)?;
            let voters: u32 = rows.iter().map(|r| r.tally.voters).sum();
            let ranged: u32 = rows.iter().map(|r| r.tally.ranged_votes).sum();
            if rows.len() >= args.min_proposals
                && voters >= args.min_voters
                && args.max_ranged.is_none_or(|m| ranged <= m)
            {
                break;
            }
        }
        if start.elapsed() > timeout {
            line("govstate", &state);
            return Err(format!("governance not done in {timeout:?}"));
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    line("govinfo", &gov.info().map_err(e)?);
    for row in all_proposals().map_err(e)? {
        line("proposal", &row);
    }
    for hash in &args.votes {
        let votes = gov.current_votes(hash.clone(), None).map_err(e)?;
        println!(
            "votes {hash} {}",
            serde_json::to_string(&votes).map_err(|e| e.to_string())?
        );
    }
    if let Some(vote) = relay {
        match engine.block_on(gov.relay_vote(vote)) {
            Ok(r) => line("relayed", &r),
            Err(err) => println!("relay_error {} {err}", err.code()),
        }
    }
    for text in &args.console {
        let mut ctx = ConsoleContext {
            session: Arc::clone(session),
            wallet: None,
            grant_id: None,
        };
        println!("console {text}");
        match engine.block_on(ctx.run(text)) {
            Ok(p) => println!("result {}", p.result.replace('\n', " ")),
            Err(ConsoleFailure::Rpc { code, message }) => println!("error {message} (code {code})"),
            Err(other) => println!("error {other}"),
        }
    }
    if let (Some(kind), Some(id)) = (&args.wait_mine, wallet) {
        let mut last = Vec::new();
        loop {
            let mine = gov.my_proposals(id).map_err(e)?;
            let kinds: Vec<String> = mine.iter().map(state_kind).collect();
            if kinds != last {
                for p in &mine {
                    line("mine", p);
                }
                last = kinds.clone();
            }
            if !kinds.is_empty() && kinds.iter().all(|k| k == kind) {
                break;
            }
            if start.elapsed() > timeout {
                return Err(format!("own proposals {kinds:?}, not all {kind:?}"));
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    if args.linger_secs > 0 {
        std::thread::sleep(Duration::from_secs(args.linger_secs));
    }
    if let Some(id) = wallet {
        for p in gov.my_proposals(id).map_err(e)? {
            line("mine", &p);
        }
    }
    line("govstate", &gov.sync_state().map_err(e)?);
    Ok(())
}
