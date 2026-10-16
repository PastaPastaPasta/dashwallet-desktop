//! The governance commands of the console (MG-02 §6): `getgovernanceinfo`,
//! `getsuperblockbudget`, `gobject count|list|get|getcurrentvotes` and
//! `voteraw`, answered from the `dw_engine::governance` facade with the
//! result shapes of Dash Core v24 (`src/rpc/governance.cpp`).
//!
//! The `*_json` functions turn the facade's records into Core's objects and
//! touch no network, so the tests build the records by hand. Vote counts are
//! the facade's estimates (MG-00 D45); the wallet tallies funding votes on
//! proposals only, so triggers, the `valid`/`delete`/`endorsed` tallies and
//! the erased-object count stay at what an SPV node can honestly say.

use std::sync::Arc;

use dw_engine::EngineError;
use dw_engine::governance::{
    CurrentVoteRow, Governance, GovernanceError, GovernanceInfo, GovernanceSyncPhase,
    ProposalDetail, ProposalQuery, ProposalScope, RawVote, VoteOutcome, superblock_budget,
};
use dw_governance::object::{display_hex, parse_display_hex};
use dw_governance::params::governance_params;
use dw_governance::vote::GovernanceVote;
use zeroize::Zeroizing;

use crate::ConsoleFailure;
use crate::exec::{ConsoleContext, arg, code, i64_of, rpc};
use crate::json::Json;

/// `ObjectType` of a proposal (`GovernanceObject::PROPOSAL`).
const OBJECT_TYPE_PROPOSAL: u32 = 1;

/// Core's text for `gobject` without a command (the RPC help).
const GOBJECT_HELP: &str = "gobject \"command\"\n\nSet of commands to manage governance objects.\n\nAvailable commands:\n  check              - Validate governance object data (proposal only)\n  prepare            - Prepare governance object by signing and creating tx\n  list-prepared      - Returns a list of governance objects prepared by this wallet with \"gobject prepare\"\n  submit             - Submit governance object to network\n  deserialize        - Deserialize governance object from hex string to JSON\n  count              - Count governance objects and votes (additional param: 'json' or 'all', default: 'json')\n  get                - Get governance object by hash\n  getcurrentvotes    - Get only current (tallying) votes for a governance object hash (does not include old votes)\n  list               - List governance objects (can be filtered by signal and/or object type)\n  diff               - List differences since last diff\n  vote-alias         - Vote on a governance object by masternode proTxHash\n  vote-many          - Vote on a governance object by all masternodes for which the voting key is in the wallet\n\nArguments:\n1. command    (string, required) The command to execute";

/// The `gobject` subcommands Core has that this console does not answer.
const UNAVAILABLE_SUBCOMMANDS: [&str; 8] = [
    "check",
    "prepare",
    "list-prepared",
    "submit",
    "deserialize",
    "diff",
    "vote-alias",
    "vote-many",
];

/// The cached signal `gobject list` filters by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListSignal {
    Valid,
    Funding,
    Delete,
    Endorsed,
    All,
}

impl ListSignal {
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "valid" => Self::Valid,
            "funding" => Self::Funding,
            "delete" => Self::Delete,
            "endorsed" => Self::Endorsed,
            "all" => Self::All,
            _ => return None,
        })
    }

    /// Whether an object with this cached funding flag passes the filter.
    /// Every listed object is cached valid; none is cached delete or
    /// endorsed (the wallet tallies funding votes only, MG-02 D21).
    fn matches(self, cached_funding: bool) -> bool {
        match self {
            Self::Valid | Self::All => true,
            Self::Funding => cached_funding,
            Self::Delete | Self::Endorsed => false,
        }
    }
}

/// The object type `gobject list` filters by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListType {
    Proposals,
    Triggers,
    All,
}

impl ListType {
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "proposals" => Self::Proposals,
            "triggers" => Self::Triggers,
            "all" => Self::All,
            _ => return None,
        })
    }

    fn includes_proposals(self) -> bool {
        self != Self::Triggers
    }
}

/// `UniValue::pushKV`: a repeated key replaces its value.
fn push_kv(fields: &mut Vec<(String, Json)>, key: &str, value: Json) {
    match fields.iter_mut().find(|(k, _)| k == key) {
        Some(slot) => slot.1 = value,
        None => fields.push((key.to_string(), value)),
    }
}

/// Core's `getgovernanceinfo`. `min_quorum` is the network's
/// `nGovernanceMinQuorum`; `fundingthreshold` is Core's plain
/// `m_valid_weighted / 10` (the facade's `passing_threshold` also applies
/// the quorum floor).
pub fn governance_info_json(info: &GovernanceInfo, min_quorum: u32) -> Json {
    Json::obj([
        ("governanceminquorum", Json::int(min_quorum)),
        ("proposalfee", Json::amount(i64_of(info.proposal_fee))),
        ("superblockcycle", Json::int(info.superblock_cycle)),
        ("superblockmaturitywindow", Json::int(info.maturity_window)),
        ("lastsuperblock", Json::int(info.clock.last_superblock)),
        ("nextsuperblock", Json::int(info.clock.next_superblock)),
        ("fundingthreshold", Json::int(info.valid_weight / 10)),
        ("governancebudget", Json::amount(i64_of(info.budget))),
    ])
}

/// Core's `CGovernanceManager::ToJson`. The wallet keeps proposals and
/// triggers but lists proposals only, so `objects_total` is the proposals
/// and `triggers`, `other` and `erased` are 0.
pub fn count_json(proposals: usize, votes: u64) -> Json {
    Json::obj([
        ("objects_total", Json::int(proposals as u64)),
        ("proposals", Json::int(proposals as u64)),
        ("triggers", Json::int(0u32)),
        ("other", Json::int(0u32)),
        ("erased", Json::int(0u32)),
        ("votes", Json::int(votes)),
    ])
}

/// Core's `CGovernanceManager::ToString` (`gobject count all`).
pub fn count_text(proposals: usize, votes: u64) -> String {
    format!(
        "Governance Objects: {proposals} (Proposals: {proposals}, Triggers: 0, Other: 0; Erased: 0), Votes: {votes}"
    )
}

/// `AbsoluteYesCount`, `YesCount`, `NoCount`, `AbstainCount` of the funding
/// tally (`GetVotesJson`), as the facade estimates them.
fn funding_counts(d: &ProposalDetail) -> [(&'static str, Json); 4] {
    let t = &d.row.tally;
    [
        ("AbsoluteYesCount", Json::int(t.net_yes_estimate)),
        ("YesCount", Json::int(t.yes.estimate)),
        ("NoCount", Json::int(t.no.estimate)),
        ("AbstainCount", Json::int(t.abstain.estimate)),
    ]
}

/// `GetStateJson`: the fields `gobject list` and `gobject get` share.
/// `cached_funding` is Core's `fCachedFunding`, absolute yes votes at or
/// above the passing threshold.
fn state_fields(
    d: &ProposalDetail,
    local_valid_key: &str,
    cached_funding: bool,
) -> Vec<(String, Json)> {
    let mut f = Vec::new();
    push_kv(
        &mut f,
        "DataHex",
        Json::str(hex::encode(d.data_json.as_bytes())),
    );
    push_kv(&mut f, "DataString", Json::str(d.data_json.clone()));
    push_kv(&mut f, "Hash", Json::str(d.row.hash.clone()));
    push_kv(
        &mut f,
        "CollateralHash",
        Json::str(d.collateral_txid.clone()),
    );
    push_kv(&mut f, "ObjectType", Json::int(OBJECT_TYPE_PROPOSAL));
    push_kv(&mut f, "CreationTime", Json::int(d.created_at));
    // The facade lists validated objects only (MG-02 §3), so they are valid
    // locally with no reason, and cached valid.
    push_kv(&mut f, local_valid_key, Json::Bool(true));
    push_kv(&mut f, "IsValidReason", Json::str(""));
    push_kv(&mut f, "fCachedValid", Json::Bool(true));
    push_kv(&mut f, "fCachedFunding", Json::Bool(cached_funding));
    push_kv(&mut f, "fCachedDelete", Json::Bool(false));
    push_kv(&mut f, "fCachedEndorsed", Json::Bool(false));
    f
}

/// Whether the proposal's absolute yes estimate reaches `threshold`.
fn cached_funding(d: &ProposalDetail, threshold: u32) -> bool {
    d.row.tally.net_yes_estimate >= i64::from(threshold)
}

/// One `gobject list` entry.
pub fn list_entry_json(d: &ProposalDetail, threshold: u32) -> Json {
    let mut f = state_fields(d, "fBlockchainValidity", cached_funding(d, threshold));
    for (k, v) in funding_counts(d) {
        push_kv(&mut f, k, v);
    }
    Json::Obj(f)
}

/// The key order of Core's `mapObjects` (`std::map<uint256, …>`): a hash
/// compares by its internal bytes, the reverse of the displayed ones.
fn core_hash_order(hash: &str) -> Vec<u8> {
    let mut bytes = hex::decode(hash).unwrap_or_default();
    bytes.reverse();
    bytes
}

/// Core's `gobject list`: the proposals that pass the filters, keyed by hash
/// in Core's order. Triggers are not offered by the facade, so `triggers`
/// lists nothing.
pub fn list_json(
    details: &[ProposalDetail],
    threshold: u32,
    signal: ListSignal,
    kind: ListType,
) -> Json {
    if !kind.includes_proposals() {
        return Json::Obj(Vec::new());
    }
    let mut keep: Vec<&ProposalDetail> = details
        .iter()
        .filter(|d| signal.matches(cached_funding(d, threshold)))
        .collect();
    keep.sort_by_cached_key(|d| core_hash_order(&d.row.hash));
    let mut out = Vec::new();
    for d in keep {
        push_kv(&mut out, &d.row.hash, list_entry_json(d, threshold));
    }
    Json::Obj(out)
}

/// Core's `gobject get`.
pub fn get_json(d: &ProposalDetail, threshold: u32) -> Json {
    let mut f = state_fields(d, "fLocalValidity", cached_funding(d, threshold));
    push_kv(
        &mut f,
        "FundingResult",
        Json::Obj(
            funding_counts(d)
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        ),
    );
    // The wallet tallies funding votes only.
    for key in ["ValidResult", "DeleteResult", "EndorsedResult"] {
        push_kv(
            &mut f,
            key,
            Json::obj([
                ("AbsoluteYesCount", Json::int(0u32)),
                ("YesCount", Json::int(0u32)),
                ("NoCount", Json::int(0u32)),
                ("AbstainCount", Json::int(0u32)),
            ]),
        );
    }
    Json::Obj(f)
}

/// Core's hash of a vote (`CGovernanceVote::GetHash`, which leaves the
/// signature out) as `row` describes it on `proposal`.
fn vote_hash(proposal: &str, row: &CurrentVoteRow) -> Option<String> {
    let (txid, vout) = row.outpoint.rsplit_once('-')?;
    let outpoint = dashcore::OutPoint::new(txid.parse().ok()?, vout.parse().ok()?);
    let vote = GovernanceVote::funding(
        outpoint,
        parse_display_hex(proposal)?,
        row.outcome.into(),
        row.time,
    );
    Some(display_hex(&vote.hash()))
}

fn outcome_name(o: VoteOutcome) -> &'static str {
    match o {
        VoteOutcome::Yes => "yes",
        VoteOutcome::No => "no",
        VoteOutcome::Abstain => "abstain",
    }
}

/// Core's `gobject getcurrentvotes`: vote hash → `outpoint:time:outcome:
/// signal:weight` (`CGovernanceVote::ToString`), the weight being the
/// facade's estimate. The facade's votes are funding votes.
pub fn current_votes_json(proposal: &str, rows: &[CurrentVoteRow]) -> Json {
    let mut out = Vec::new();
    for r in rows {
        let key = vote_hash(proposal, r).unwrap_or_else(|| r.outpoint.clone());
        push_kv(
            &mut out,
            &key,
            Json::str(format!(
                "{}:{}:{}:funding:{}",
                r.outpoint,
                r.time,
                outcome_name(r.outcome),
                r.weight.estimate
            )),
        );
    }
    Json::Obj(out)
}

/// Core's `ParseHashV`: 64 hex characters. Returns the hash in lower case.
fn parse_hash_v(text: &str, name: &str) -> Result<String, ConsoleFailure> {
    if text.len() != 64 {
        return Err(rpc(
            code::INVALID_PARAMETER,
            format!(
                "{name} must be of length 64 (not {}, for '{text}')",
                text.len()
            ),
        ));
    }
    if !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(rpc(
            code::INVALID_PARAMETER,
            format!("{name} must be hexadecimal string (not '{text}')"),
        ));
    }
    Ok(text.to_ascii_lowercase())
}

/// `UniValue::getInt<int>` of a typed argument.
fn get_int(text: &str) -> Result<i32, ConsoleFailure> {
    let n: i64 = text.trim().parse().map_err(|_| {
        rpc(
            code::TYPE_ERROR,
            format!("JSON value is not an integer as expected: {text}"),
        )
    })?;
    i32::try_from(n).map_err(|_| rpc(code::TYPE_ERROR, "JSON integer out of range"))
}

/// The error of a governance command while governance sync is off. Core has
/// no such RPC error (`-disablegovernance` leaves an empty manager); the
/// console says so instead of answering from an empty one.
pub fn disabled() -> ConsoleFailure {
    rpc(
        code::MISC_ERROR,
        "Governance is disabled: governance sync is turned off. Turn it on in the governance settings and try again.",
    )
}

fn phase_name(phase: GovernanceSyncPhase) -> String {
    serde_json::to_value(phase)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// `gobject get` and `getcurrentvotes` word an unknown object differently.
fn failure(e: GovernanceError, unknown_object: &str) -> ConsoleFailure {
    match e {
        GovernanceError::SyncDisabled => disabled(),
        GovernanceError::NotSynced { phase } => rpc(
            code::CLIENT_IN_INITIAL_DOWNLOAD,
            format!(
                "Must wait for governance data to sync with the masternode network (phase: {}). Try again in a minute or so.",
                phase_name(phase)
            ),
        ),
        GovernanceError::ProposalNotFound { .. } => rpc(code::INVALID_PARAMETER, unknown_object),
        GovernanceError::ListUnavailable => rpc(
            code::MISC_ERROR,
            "The masternode list is not available yet. Try again in a minute or so.",
        ),
        GovernanceError::NotImplemented { call } => ConsoleFailure::NotAvailable(call),
        GovernanceError::VoteRejected { reason } => vote_rejection(&reason),
        GovernanceError::NoPeers => no_peers(),
        GovernanceError::Engine(e) => crate::exec::engine_failure(e),
    }
}

fn no_peers() -> ConsoleFailure {
    rpc(
        code::CLIENT_NOT_CONNECTED,
        "The vote is stored, but no governance peer is connected. It will be relayed when a peer connects.",
    )
}

/// Core's `voteraw` error for a vote the facade refused (`reason` is the
/// facade's snake-case rejection).
fn vote_rejection(reason: &str) -> ConsoleFailure {
    let internal = |msg: &str| rpc(code::INTERNAL_ERROR, msg);
    match reason {
        "unknown_object" => rpc(code::INVALID_PARAMETER, "Governance object not found"),
        // `IsValid` failed: the signature does not recover the voting key,
        // or the time is over an hour ahead.
        "bad_signature" | "wrong_voting_key" | "unknown_voting_key" | "malformed"
        | "too_far_ahead" => internal("Failure to verify vote."),
        "duplicate" => {
            internal("Error voting : CGovernanceObject::ProcessVote -- Already known valid vote")
        }
        "obsolete" => internal("Error voting : CGovernanceObject::ProcessVote -- Obsolete vote"),
        // Core logs this one and leaves the exception empty.
        "expired" => internal("Error voting : "),
        "key_limit" => internal("Error voting : too many votes seen for this voting key"),
        "not_a_proposal" => {
            internal("Error voting : only votes on proposals are supported in SPV mode")
        }
        other => internal(&format!("Error voting : {other}")),
    }
}

/// `subcommand`'s argument count check; Core answers a wrong count with
/// the command's usage.
fn sub_arity(
    rest: &[Zeroizing<String>],
    min: usize,
    max: usize,
    usage: &str,
) -> Result<(), ConsoleFailure> {
    if rest.len() < min || rest.len() > max {
        Err(rpc(code::MISC_ERROR, usage))
    } else {
        Ok(())
    }
}

impl ConsoleContext {
    pub(crate) async fn governance_command(
        &self,
        method: &str,
        args: &[Zeroizing<String>],
    ) -> Result<Json, ConsoleFailure> {
        let g = Arc::clone(&self.session).governance();
        match method {
            "getgovernanceinfo" => {
                crate::exec::arity(method, args, 0, 0)?;
                require_enabled(&g)?;
                let info = g.info().map_err(|e| failure(e, ""))?;
                let quorum = governance_params(self.session.network().core_network()).min_quorum;
                Ok(governance_info_json(&info, quorum))
            }
            "getsuperblockbudget" => {
                crate::exec::arity(method, args, 1, 1)?;
                let height = get_int(arg(args, 0).unwrap_or_default())?;
                let height = u32::try_from(height)
                    .map_err(|_| rpc(code::INVALID_PARAMETER, "Block height out of range"))?;
                // Core reads the chain only, not the governance manager, so
                // this answers with governance sync off too.
                Ok(Json::amount(i64_of(superblock_budget(
                    self.session.network(),
                    height,
                ))))
            }
            "gobject" => self.gobject(&g, args),
            "voteraw" => self.voteraw(&g, args).await,
            other => Err(ConsoleFailure::NotAvailable(other.to_string())),
        }
    }

    fn gobject(&self, g: &Governance, args: &[Zeroizing<String>]) -> Result<Json, ConsoleFailure> {
        let Some(sub) = arg(args, 0) else {
            return Err(rpc(code::MISC_ERROR, GOBJECT_HELP));
        };
        let rest = &args[1..];
        match sub {
            "count" => {
                sub_arity(rest, 0, 1, "gobject count ( \"mode\" )")?;
                let mode = arg(rest, 0).unwrap_or("json");
                if mode != "json" && mode != "all" {
                    return Err(rpc(code::INVALID_PARAMETER, "mode can be 'json' or 'all'"));
                }
                require_enabled(g)?;
                let proposals = g
                    .proposals(ProposalQuery {
                        scope: ProposalScope::All,
                        text: None,
                    })
                    .map_err(|e| failure(e, ""))?
                    .len();
                let votes = g.sync_state().map_err(|e| failure(e, ""))?.votes;
                Ok(if mode == "json" {
                    count_json(proposals, votes)
                } else {
                    Json::Str(count_text(proposals, votes))
                })
            }
            "list" => {
                sub_arity(rest, 0, 2, "gobject list ( \"signal\" \"type\" )")?;
                // Core answers a bad filter with a string, not an error.
                let Some(signal) = ListSignal::parse(arg(rest, 0).unwrap_or("valid")) else {
                    return Ok(Json::str(
                        "Invalid signal, should be 'valid', 'funding', 'delete', 'endorsed' or 'all'",
                    ));
                };
                let Some(kind) = ListType::parse(arg(rest, 1).unwrap_or("all")) else {
                    return Ok(Json::str(
                        "Invalid type, should be 'proposals', 'triggers' or 'all'",
                    ));
                };
                require_enabled(g)?;
                let rows = g
                    .proposals(ProposalQuery {
                        scope: ProposalScope::All,
                        text: None,
                    })
                    .map_err(|e| failure(e, ""))?;
                let threshold = g.info().map_err(|e| failure(e, ""))?.passing_threshold;
                let mut details = Vec::new();
                for row in rows {
                    match g.proposal(row.hash) {
                        Ok(d) => details.push(d),
                        // Dropped between the two reads.
                        Err(GovernanceError::ProposalNotFound { .. }) => {}
                        Err(e) => return Err(failure(e, "")),
                    }
                }
                Ok(list_json(&details, threshold, signal, kind))
            }
            "get" => {
                sub_arity(rest, 1, 1, "gobject get \"governance-hash\"")?;
                let hash = parse_hash_v(arg(rest, 0).unwrap_or_default(), "GovObj hash")?;
                require_enabled(g)?;
                let detail = g
                    .proposal(hash)
                    .map_err(|e| failure(e, "Unknown governance object"))?;
                let threshold = g.info().map_err(|e| failure(e, ""))?.passing_threshold;
                Ok(get_json(&detail, threshold))
            }
            "getcurrentvotes" => {
                sub_arity(
                    rest,
                    1,
                    3,
                    "gobject getcurrentvotes \"governance-hash\" ( \"txid\" \"vout\" )",
                )?;
                let hash = parse_hash_v(arg(rest, 0).unwrap_or_default(), "Governance hash")?;
                // Core filters only when both txid and vout are given.
                let outpoint = match (arg(rest, 1), arg(rest, 2)) {
                    (Some(txid), Some(vout)) => {
                        let txid = parse_hash_v(txid, "Masternode Collateral hash")?;
                        // `LocaleIndependentAtoi<uint32_t>`: no digits read as 0.
                        let vout = vout.trim().parse::<u32>().unwrap_or(0);
                        Some(format!("{txid}-{vout}"))
                    }
                    _ => None,
                };
                require_enabled(g)?;
                let rows = g
                    .current_votes(hash.clone(), outpoint)
                    .map_err(|e| failure(e, "Unknown governance-hash"))?;
                Ok(current_votes_json(&hash, &rows))
            }
            other if UNAVAILABLE_SUBCOMMANDS.contains(&other) => {
                Err(ConsoleFailure::NotAvailable(format!("gobject {other}")))
            }
            _ => Err(rpc(code::INVALID_PARAMETER, "Must be a valid command")),
        }
    }

    async fn voteraw(
        &self,
        g: &Governance,
        args: &[Zeroizing<String>],
    ) -> Result<Json, ConsoleFailure> {
        crate::exec::arity("voteraw", args, 7, 7)?;
        let txid = parse_hash_v(arg(args, 0).unwrap_or_default(), "mn collateral tx hash")?;
        let index = get_int(arg(args, 1).unwrap_or_default())?;
        let proposal = parse_hash_v(arg(args, 2).unwrap_or_default(), "Governance hash")?;
        let signal = arg(args, 3).unwrap_or_default();
        let outcome = arg(args, 4).unwrap_or_default();
        if !["funding", "valid", "delete", "endorsed"].contains(&signal) {
            return Err(rpc(
                code::INVALID_PARAMETER,
                "Invalid vote signal. Please use one of the following: (funding|valid|delete|endorsed)",
            ));
        }
        let outcome = match outcome {
            "yes" => VoteOutcome::Yes,
            "no" => VoteOutcome::No,
            "abstain" => VoteOutcome::Abstain,
            _ => {
                return Err(rpc(
                    code::INVALID_PARAMETER,
                    "Invalid vote outcome. Please use one of the following: 'yes', 'no' or 'abstain'",
                ));
            }
        };
        // Core relays any of the four signals; the wallet tallies, and so
        // accepts, funding votes only (MG-02 D21).
        if signal != "funding" {
            return Err(ConsoleFailure::NotAvailable(format!(
                "voteraw with signal {signal}"
            )));
        }
        let time: i64 = arg(args, 5)
            .unwrap_or_default()
            .trim()
            .parse()
            .map_err(|_| {
                rpc(
                    code::TYPE_ERROR,
                    format!(
                        "JSON value is not an integer as expected: {}",
                        arg(args, 5).unwrap_or_default()
                    ),
                )
            })?;
        require_enabled(g)?;
        g.relay_vote(RawVote {
            // Core's `COutPoint(hash, int)` reads the index as unsigned.
            collateral: format!("{txid}-{}", index as u32),
            proposal,
            outcome,
            time,
            signature: arg(args, 6).unwrap_or_default().to_string(),
        })
        .await
        .map_err(|e| match e {
            // The signature is the only argument the facade parses itself.
            GovernanceError::Engine(EngineError::InvalidArgument(_)) => {
                rpc(code::INVALID_ADDRESS_OR_KEY, "Malformed base64 encoding")
            }
            other => failure(other, "Governance object not found"),
        })?;
        Ok(Json::str("Voted successfully"))
    }
}

fn require_enabled(g: &Governance) -> Result<(), ConsoleFailure> {
    if g.sync_enabled().map_err(|e| failure(e, ""))? {
        Ok(())
    } else {
        Err(disabled())
    }
}
