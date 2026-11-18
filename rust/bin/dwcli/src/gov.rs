//! `dwcli gov …`: the governance facade (MG-08, `docs/contracts/mg-engine.md`),
//! keeping Core's terms (`docs/design/MG-00-masternodes-ux.md` row MG-08).
//! Every command prints one JSON line on stdout (`{"ok":true,"result":…}` or
//! `{"ok":false,"error":{"code":…,"message":…}}` with exit status 1) after
//! SPV stops and the engine shuts down, as `dwcli mn` does (MG-03 D26, D32).
//! No error repeats a caller's argument, and nothing signs as operator.

use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::{ArgAction, Args, Subcommand};
use dw_engine::governance::{
    Governance, GovernanceError, GovernanceSyncPhase, ProposalDraft, ProposalQuery, ProposalScope,
    RawVote, VoteOutcome, superblock_budget,
};
use dw_engine::{Engine, EngineError, NetworkSession};
use dw_vault::GrantPurpose;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::governance::proposal_grant_cap;
use crate::masternodes::{Failure, snake, value, wait_for_list, wallet};

#[derive(Args)]
pub struct GovArgs {
    /// Start SPV and wait for the masternode list and governance tip before
    /// the command; SPV stops after it.
    #[arg(long, global = true)]
    spv: bool,
    /// Turn governance sync on and wait until it reaches `synced` (implies
    /// `--spv`).
    #[arg(long, global = true)]
    gov: bool,
    /// Seconds `--spv` and `--gov` wait.
    #[arg(long, global = true, default_value_t = 120)]
    sync_timeout: u64,
    /// With `--spv` or `--gov`, also wait until the tip and every loaded
    /// wallet scanned this height (implies `--spv`).
    #[arg(long, global = true)]
    sync_height: Option<u32>,
    /// With `--gov`, wait until at least this many proposals are listed
    /// (implies `--gov`).
    #[arg(long, global = true, default_value_t = 0)]
    min_proposals: usize,
    /// With `--gov`, wait until the listed proposals' voters add up to at
    /// least this many (implies `--gov`).
    #[arg(long, global = true, default_value_t = 0)]
    min_voters: u32,
    /// With `--gov`, wait until the listed proposals' ranged votes add up to
    /// at most this many (implies `--gov`).
    #[arg(long, global = true)]
    max_ranged: Option<u32>,
    /// Seconds to stay connected after a command that started SPV, so peers
    /// fetch a relayed vote or proposal.
    #[arg(long, global = true, default_value_t = 0)]
    linger_secs: u64,
    #[command(subcommand)]
    command: GovCommand,
}

#[derive(Subcommand)]
enum GovCommand {
    /// Whether governance sync is enabled for this network.
    SyncEnabled,
    /// Persist whether governance sync is enabled (`true` or `false`), and
    /// start or stop the sync task.
    SetSyncEnabled {
        #[arg(action = ArgAction::Set)]
        enabled: bool,
    },
    /// The governance sync phase and counters.
    SyncState,
    /// Start a new governance sync run now (refused while sync is disabled).
    Resync,
    /// The governance cycle clock at the tip.
    Clock,
    /// Governance parameters, clock, superblock budget, valid masternodes and
    /// (once synced) proposal counts (`getgovernanceinfo`).
    Info,
    /// The superblock budget in duffs at `height`, or 0 for a non-superblock
    /// (`getsuperblockbudget`).
    Budget { height: u32 },
    /// Upcoming superblocks with ETAs, budgets and voting cutoffs.
    Superblocks {
        #[arg(long, default_value_t = 12)]
        count: u32,
    },
    /// Confirmed proposals (`gobject list`), filtered by `--scope` and
    /// optional `--text`.
    List {
        /// `all` or `current` (end epoch not past).
        #[arg(long, default_value = "all", value_parser = parse_scope)]
        scope: ProposalScope,
        #[arg(long)]
        text: Option<String>,
    },
    /// One confirmed proposal in full (`gobject get`).
    Get { hash: String },
    /// Current votes on a confirmed proposal (`gobject getcurrentvotes`).
    CurrentVotes {
        hash: String,
        /// Optional `<txid>-<n>` collateral outpoint filter.
        #[arg(long)]
        outpoint: Option<String>,
    },
    /// Listed masternodes whose voting key the wallet derives (indexes 0–19)
    /// and their current vote on `proposal`.
    VotingMasternodes {
        proposal: String,
        #[arg(long)]
        wallet: Option<String>,
    },
    /// Sign and relay a funding vote on `proposal` for each `--masternode`
    /// under one `Governance` grant (`gobject vote-many` / `vote-alias`).
    Vote {
        proposal: String,
        /// `yes`, `no` or `abstain`.
        #[arg(long, value_parser = parse_outcome)]
        outcome: VoteOutcome,
        #[arg(long = "masternode", required = true)]
        masternodes: Vec<String>,
        #[arg(long)]
        wallet: Option<String>,
    },
    /// Check, store and relay an externally signed funding vote (`voteraw`).
    Voteraw {
        /// `<txid>-<n>` collateral outpoint.
        collateral: String,
        proposal: String,
        /// `yes`, `no` or `abstain`.
        #[arg(long, value_parser = parse_outcome)]
        outcome: VoteOutcome,
        #[arg(long)]
        time: i64,
        /// Base64 signature.
        #[arg(long)]
        signature: String,
    },
    /// Validate a proposal and pay its 1 DASH collateral fee (`gobject
    /// prepare`), or only preview it with `--preview`.
    Prepare(Box<PrepareArgs>),
    /// The wallet's own proposals (`gobject list-prepared`), newest first.
    ListPrepared {
        #[arg(long)]
        wallet: Option<String>,
    },
    /// Broadcast a pending proposal's saved fee again or retry its
    /// announcement (`gobject submit`).
    Submit {
        hash: String,
        #[arg(long)]
        wallet: Option<String>,
    },
}

#[derive(Args)]
struct PrepareArgs {
    #[arg(long)]
    name: String,
    #[arg(long)]
    url: String,
    #[arg(long)]
    payment_address: String,
    /// Per payment, in duffs.
    #[arg(long)]
    payment_amount: u64,
    #[arg(long)]
    payment_count: u32,
    #[arg(long)]
    first_superblock: u32,
    /// Coin-control outpoint `<txid>-<n>`; repeatable.
    #[arg(long = "coin")]
    coins: Vec<String>,
    /// Validate and estimate the fee without reserving, signing or saving.
    #[arg(long)]
    preview: bool,
    #[arg(long)]
    wallet: Option<String>,
}

impl PrepareArgs {
    fn draft(&self) -> ProposalDraft {
        ProposalDraft {
            name: self.name.clone(),
            url: self.url.clone(),
            payment_address: self.payment_address.clone(),
            payment_amount: self.payment_amount,
            payment_count: self.payment_count,
            first_superblock_height: self.first_superblock,
            coins: self.coins.clone(),
        }
    }
}

impl GovArgs {
    pub fn starts_spv(&self) -> bool {
        self.spv || self.starts_gov() || self.sync_height.is_some()
    }

    fn starts_gov(&self) -> bool {
        self.gov || self.min_proposals > 0 || self.min_voters > 0 || self.max_ranged.is_some()
    }
}

fn parse_scope(s: &str) -> Result<ProposalScope, String> {
    match s {
        "all" => Ok(ProposalScope::All),
        "current" => Ok(ProposalScope::Current),
        _ => Err("all or current".into()),
    }
}

fn parse_outcome(s: &str) -> Result<VoteOutcome, String> {
    match s {
        "yes" => Ok(VoteOutcome::Yes),
        "no" => Ok(VoteOutcome::No),
        "abstain" => Ok(VoteOutcome::Abstain),
        _ => Err("one of yes, no, abstain".into()),
    }
}

fn gov_failure(e: GovernanceError) -> Failure {
    let code = match e.engine() {
        Some(EngineError::Send(f)) => format!("send.{}", snake(&format!("{f:?}"))),
        _ => e.code().into(),
    };
    let message = match &e {
        GovernanceError::ProposalNotFound { .. } => "proposal not found".into(),
        GovernanceError::Engine(EngineError::InvalidArgument(_)) => "invalid argument".into(),
        GovernanceError::Engine(EngineError::WalletNotFound(_)) => {
            "wallet not found: the requested wallet".into()
        }
        GovernanceError::Engine(EngineError::Send(
            dw_engine::SendFailure::OutpointUnavailable(_),
        )) => "send: an outpoint is unavailable".into(),
        other => other.to_string(),
    };
    Failure { code, message }
}

fn wait_for_spv(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    gov: &Governance,
    timeout: Duration,
    height: Option<u32>,
) -> Result<(), Failure> {
    let deadline = Instant::now() + timeout;
    wait_for_list(engine, session, &session.masternodes(), timeout, height).map_err(|f| {
        if f.code == "masternodes.list_unavailable" {
            gov_failure(GovernanceError::ListUnavailable)
        } else {
            f
        }
    })?;
    // Governance's follower feeds its tip and list snapshot once a second.
    loop {
        if let Ok(info) = gov.info()
            && info.valid_masternodes > 0
            && height.is_none_or(|h| info.clock.tip_height >= h)
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(gov_failure(GovernanceError::ListUnavailable));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn wait_for_gov(
    engine: &Engine,
    gov: &Governance,
    deadline: Instant,
    min_proposals: usize,
    min_voters: u32,
    max_ranged: Option<u32>,
) -> Result<(), Failure> {
    engine
        .block_on(gov.set_sync_enabled(true))
        .map_err(gov_failure)?;
    loop {
        let state = gov.sync_state().map_err(gov_failure)?;
        if state.phase == GovernanceSyncPhase::Synced {
            let rows = gov
                .proposals(ProposalQuery {
                    scope: ProposalScope::All,
                    text: None,
                })
                .map_err(gov_failure)?;
            let voters: u32 = rows.iter().map(|r| r.tally.voters).sum();
            let ranged: u32 = rows.iter().map(|r| r.tally.ranged_votes).sum();
            if rows.len() >= min_proposals
                && voters >= min_voters
                && max_ranged.is_none_or(|m| ranged <= m)
            {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            return Err(Failure {
                code: "governance.not_synced".into(),
                message: "governance did not sync within --sync-timeout".into(),
            });
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn command(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    passphrase: Option<&Zeroizing<Vec<u8>>>,
    args: GovArgs,
) -> Result<Value, Failure> {
    let gov = session.governance();
    let started_spv = args.starts_spv();
    let linger_secs = args.linger_secs;
    if started_spv {
        let timeout = Duration::from_secs(args.sync_timeout);
        let deadline = Instant::now() + timeout;
        wait_for_spv(engine, session, &gov, timeout, args.sync_height)?;
        if args.starts_gov() {
            wait_for_gov(
                engine,
                &gov,
                deadline,
                args.min_proposals,
                args.min_voters,
                args.max_ranged,
            )?;
        }
    }
    let out = match args.command {
        GovCommand::SyncEnabled => {
            value(json!({ "enabled": gov.sync_enabled().map_err(gov_failure)? }))
        }
        GovCommand::SetSyncEnabled { enabled } => {
            engine
                .block_on(gov.set_sync_enabled(enabled))
                .map_err(gov_failure)?;
            value(json!({ "enabled": gov.sync_enabled().map_err(gov_failure)? }))
        }
        GovCommand::SyncState => value(gov.sync_state().map_err(gov_failure)?),
        GovCommand::Resync => {
            gov.resync().map_err(gov_failure)?;
            value(json!({ "resync": true }))
        }
        GovCommand::Clock => value(gov.clock().map_err(gov_failure)?),
        GovCommand::Info => value(gov.info().map_err(gov_failure)?),
        GovCommand::Budget { height } => value(json!({
            "height": height,
            "budget": superblock_budget(session.network(), height),
        })),
        GovCommand::Superblocks { count } => {
            value(gov.upcoming_superblocks(count).map_err(gov_failure)?)
        }
        GovCommand::List { scope, text } => value(
            gov.proposals(ProposalQuery { scope, text })
                .map_err(gov_failure)?,
        ),
        GovCommand::Get { hash } => value(gov.proposal(hash).map_err(gov_failure)?),
        GovCommand::CurrentVotes { hash, outpoint } => {
            value(gov.current_votes(hash, outpoint).map_err(gov_failure)?)
        }
        GovCommand::VotingMasternodes {
            proposal,
            wallet: w,
        } => {
            let w = wallet(session, w)?;
            value(
                engine
                    .block_on(gov.voting_masternodes(w, proposal))
                    .map_err(gov_failure)?,
            )
        }
        GovCommand::Vote {
            proposal,
            outcome,
            masternodes,
            wallet: w,
        } => {
            let w = wallet(session, w)?;
            let grant = session
                .vault()
                .authorize(
                    GrantPurpose::Governance,
                    Some(&w.0),
                    crate::pay::credential(session, passphrase),
                )
                .map_err(|e| gov_failure(EngineError::from(e).into()))?;
            value(
                engine
                    .block_on(gov.vote(w, proposal, outcome, masternodes, grant.id))
                    .map_err(gov_failure)?,
            )
        }
        GovCommand::Voteraw {
            collateral,
            proposal,
            outcome,
            time,
            signature,
        } => {
            let vote = RawVote {
                collateral,
                proposal,
                outcome,
                time,
                signature,
            };
            value(engine.block_on(gov.relay_vote(vote)).map_err(gov_failure)?)
        }
        GovCommand::Prepare(a) => {
            let w = wallet(session, a.wallet.clone())?;
            let draft = a.draft();
            let preview = engine
                .block_on(gov.preview_proposal(w, draft.clone()))
                .map_err(gov_failure)?;
            if a.preview {
                return value(preview);
            }
            let max_duffs = proposal_grant_cap(&preview);
            let grant = session
                .vault()
                .authorize(
                    GrantPurpose::Spend { max_duffs },
                    Some(&w.0),
                    crate::pay::credential(session, passphrase),
                )
                .map_err(|e| gov_failure(EngineError::from(e).into()))?;
            value(
                engine
                    .block_on(gov.create_proposal(w, draft, grant.id))
                    .map_err(gov_failure)?,
            )
        }
        GovCommand::ListPrepared { wallet: w } => {
            let w = wallet(session, w)?;
            value(gov.my_proposals(w).map_err(gov_failure)?)
        }
        GovCommand::Submit { hash, wallet: w } => {
            let w = wallet(session, w)?;
            value(
                engine
                    .block_on(gov.submit_proposal(w, hash))
                    .map_err(gov_failure)?,
            )
        }
    };
    if out.is_ok() && started_spv && linger_secs > 0 {
        std::thread::sleep(Duration::from_secs(linger_secs));
    }
    out
}

pub fn run(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    passphrase: Option<&Zeroizing<Vec<u8>>>,
    args: GovArgs,
) -> Result<Value, Failure> {
    crate::unlock(engine, session, passphrase)
        .map_err(Failure::from)
        .and_then(|()| command(engine, session, passphrase, args))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn gov(args: &[&str]) -> Result<GovArgs, clap::error::ErrorKind> {
        let cli = crate::Cli::try_parse_from(
            ["dwcli", "--datadir", "d", "gov"]
                .into_iter()
                .chain(args.iter().copied()),
        )
        .map_err(|e| e.kind())?;
        match cli.command {
            crate::Command::Gov(a) => Ok(a),
            _ => panic!("not a `gov` command"),
        }
    }

    const HASH: &str = "ab";

    #[test]
    fn every_gov_subcommand_parses_with_its_required_arguments() {
        let prepare = [
            "prepare",
            "--name",
            "dwd_p0",
            "--url",
            "https://example.org/p0",
            "--payment-address",
            "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n",
            "--payment-amount",
            "100000000",
            "--payment-count",
            "2",
            "--first-superblock",
            "1500",
        ];
        for args in [
            &["sync-enabled"][..],
            &["set-sync-enabled", "true"],
            &["set-sync-enabled", "false"],
            &["sync-state"],
            &["resync"],
            &["clock"],
            &["info"],
            &["budget", "1500"],
            &["superblocks"],
            &["superblocks", "--count", "6"],
            &["list"],
            &["list", "--scope", "current", "--text", "dwd"],
            &["get", HASH],
            &["current-votes", HASH],
            &["current-votes", HASH, "--outpoint", "cd-0"],
            &["voting-masternodes", HASH],
            &["voting-masternodes", HASH, "--wallet", "w"],
            &["vote", HASH, "--outcome", "yes", "--masternode", HASH],
            &[
                "voteraw",
                "cd-0",
                HASH,
                "--outcome",
                "abstain",
                "--time",
                "1700000000",
                "--signature",
                "c2ln",
            ],
            &prepare,
            &[
                "prepare",
                "--preview",
                "--name",
                "dwd_p0",
                "--url",
                "https://example.org/p0",
                "--payment-address",
                "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n",
                "--payment-amount",
                "100000000",
                "--payment-count",
                "1",
                "--first-superblock",
                "1500",
                "--coin",
                "cd-0",
            ],
            &["list-prepared"],
            &["submit", HASH],
        ] {
            assert!(gov(args).is_ok(), "{args:?}: {:?}", gov(args).err());
        }
    }

    #[test]
    fn gov_subcommands_refuse_missing_or_malformed_arguments() {
        use clap::error::ErrorKind as K;
        assert_eq!(
            gov(&[]).err(),
            Some(K::DisplayHelpOnMissingArgumentOrSubcommand)
        );
        for (args, kind) in [
            (&["set-sync-enabled"][..], K::MissingRequiredArgument),
            (&["set-sync-enabled", "on"], K::InvalidValue),
            (&["budget"], K::MissingRequiredArgument),
            (&["budget", "-1"], K::UnknownArgument),
            (&["list", "--scope", "mine"], K::ValueValidation),
            (&["get"], K::MissingRequiredArgument),
            (
                &["vote", HASH, "--outcome", "yes"],
                K::MissingRequiredArgument,
            ),
            (
                &["vote", HASH, "--outcome", "maybe", "--masternode", HASH],
                K::ValueValidation,
            ),
            (&["voteraw", "cd-0", HASH], K::MissingRequiredArgument),
            (&["prepare", "--name", "p"], K::MissingRequiredArgument),
            (&["submit"], K::MissingRequiredArgument),
        ] {
            assert_eq!(gov(args).err(), Some(kind), "{args:?}");
        }
    }

    #[test]
    fn gov_failures_keep_contract_codes_and_redact_bearer_shaped_inputs() {
        let secret = "e9873d79c6d87dc0fb6a5778633389f4453213303da61f20bd67fc233aa33262";
        let outpoint: dashcore::OutPoint = format!("{secret}:0").parse().unwrap();
        for (err, want) in [
            (
                GovernanceError::ProposalNotFound {
                    hash: secret.into(),
                },
                "governance.proposal_not_found",
            ),
            (
                GovernanceError::Engine(EngineError::InvalidArgument(secret.into())),
                "invalid_argument",
            ),
            (
                GovernanceError::Engine(EngineError::WalletNotFound(secret.into())),
                "wallet_not_found",
            ),
            (
                GovernanceError::Engine(EngineError::Send(
                    dw_engine::SendFailure::OutpointUnavailable(outpoint),
                )),
                "send.outpoint_unavailable",
            ),
        ] {
            let f = gov_failure(err);
            assert_eq!(f.code, want);
            assert!(!f.message.contains(secret), "{}", f.message);
        }
    }
}
