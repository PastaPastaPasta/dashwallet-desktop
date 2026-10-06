//! Governance console commands (M3 R2, QT-145): `getgovernanceinfo`,
//! `getsuperblockbudget` and the `gobject` subcommands an SPV wallet can
//! answer from synced objects (`list`, `get`, `getcurrentvotes`, `count`)
//! or carry out (`vote-many`). Result shapes are Core's
//! (`src/rpc/governance.cpp`), limited to the fields the wallet knows.
//! `gobject prepare/submit/check/diff/vote-alias/…` answer "not available"
//! (the Create Proposal wizard and `dwcli gov` cover creating proposals).

use std::sync::Arc;

use dw_engine::governance::{ProposalQuery, ProposalSource, VoteOutcome};
use dw_engine::{EngineError, NetworkSession};
use dw_vault::GrantPurpose;
use zeroize::Zeroizing;

use crate::exec::ConsoleContext;
use crate::json::Json;
use crate::{ConsoleFailure, command};

const INVALID_PARAMETER: i32 = -8;
const MISC_ERROR: i32 = -1;
const COIN: u64 = 100_000_000;

/// The commands this module answers.
pub(crate) fn handles(method: &str) -> bool {
    matches!(method, "gobject" | "getgovernanceinfo" | "getsuperblockbudget")
}

fn rpc(code: i32, message: impl Into<String>) -> ConsoleFailure {
    ConsoleFailure::Rpc {
        code,
        message: message.into(),
    }
}

fn engine(e: EngineError) -> ConsoleFailure {
    match e {
        EngineError::Governance(dw_engine::GovernanceFailure::ProposalNotFound(_)) => {
            rpc(INVALID_PARAMETER, "Unknown governance object")
        }
        EngineError::Governance(dw_engine::GovernanceFailure::SyncDisabled) => rpc(
            MISC_ERROR,
            "Governance sync is off (enable the Governance tab or clock)",
        ),
        EngineError::Governance(dw_engine::GovernanceFailure::NotSynced) => {
            rpc(MISC_ERROR, "Governance data is not synced yet")
        }
        other => ConsoleFailure::Engine(other),
    }
}

fn usage(name: &str) -> ConsoleFailure {
    rpc(MISC_ERROR, command(name).map(|c| c.usage).unwrap_or(name))
}

fn amount(duffs: u64) -> Json {
    Json::amount(i64::try_from(duffs).unwrap_or(i64::MAX))
}

fn arg(args: &[Zeroizing<String>], i: usize) -> Option<&str> {
    args.get(i).map(|a| a.as_str())
}

impl ConsoleContext {
    pub(crate) async fn governance(
        &mut self,
        method: &str,
        args: &[Zeroizing<String>],
    ) -> Result<Json, ConsoleFailure> {
        let s = Arc::clone(&self.session);
        match method {
            "getgovernanceinfo" => {
                if !args.is_empty() {
                    return Err(usage(method));
                }
                let p = dw_governance_params(&s);
                let info = s.governance_info().await.map_err(engine)?;
                let (Some(last), Some(next), Some(budget)) =
                    (info.last_superblock, info.next_superblock, info.budget_available)
                else {
                    return Err(rpc(MISC_ERROR, "Block height is not known yet"));
                };
                let mut f = vec![
                    ("governanceminquorum".to_string(), Json::int(p.0)),
                    ("proposalfee".to_string(), amount(COIN)),
                    ("superblockcycle".to_string(), Json::int(info.superblock_cycle)),
                    ("superblockmaturitywindow".to_string(), Json::int(p.1)),
                    ("lastsuperblock".to_string(), Json::int(last)),
                    ("nextsuperblock".to_string(), Json::int(next)),
                ];
                // Core: valid weighted masternodes / 10 (the list, not a guess).
                if let (Some(mn), Some(evo)) = (info.masternodes_eligible, info.evonodes_eligible) {
                    f.push(("fundingthreshold".to_string(), Json::int((mn + 4 * evo) / 10)));
                }
                f.push(("governancebudget".to_string(), amount(budget)));
                Ok(Json::Obj(f))
            }
            "getsuperblockbudget" => {
                let Some(h) = arg(args, 0).filter(|_| args.len() == 1) else {
                    return Err(usage(method));
                };
                let height: u32 = h
                    .trim()
                    .parse()
                    .map_err(|_| rpc(INVALID_PARAMETER, "Block height out of range"))?;
                Ok(amount(s.superblock_budget(height)))
            }
            _ => self.gobject(&s, args).await,
        }
    }

    async fn gobject(
        &mut self,
        s: &Arc<NetworkSession>,
        args: &[Zeroizing<String>],
    ) -> Result<Json, ConsoleFailure> {
        let Some(sub) = arg(args, 0) else {
            return Err(usage("gobject"));
        };
        match sub {
            "list" | "count" => {
                let rows = s
                    .proposals(ProposalQuery {
                        source: ProposalSource::Active,
                        title_filter: None,
                    })
                    .await
                    .map_err(engine)?;
                if sub == "count" {
                    let votes = s.governance_sync_state().map_err(engine)?.votes;
                    return Ok(Json::obj([
                        ("objects_total", Json::int(rows.len() as u64)),
                        ("proposals", Json::int(rows.len() as u64)),
                        ("votes", Json::int(votes)),
                    ]));
                }
                let mut out = Vec::with_capacity(rows.len());
                for r in rows {
                    let d = s.proposal_detail(r.hash.clone()).await.map_err(engine)?;
                    out.push((r.hash.clone(), object_json(&d)));
                }
                Ok(Json::Obj(out))
            }
            "get" => {
                let Some(hash) = arg(args, 1).filter(|_| args.len() == 2) else {
                    return Err(usage("gobject"));
                };
                let d = s.proposal_detail(hash.to_string()).await.map_err(engine)?;
                Ok(object_json(&d))
            }
            "getcurrentvotes" => {
                let Some(hash) = arg(args, 1).filter(|_| args.len() == 2) else {
                    return Err(usage("gobject"));
                };
                let votes = s.governance_current_votes(hash).map_err(engine)?;
                Ok(Json::Obj(
                    votes.into_iter().map(|(h, v)| (h, Json::Str(v))).collect(),
                ))
            }
            "vote-many" => {
                if args.len() != 4 {
                    return Err(usage("gobject"));
                }
                let (hash, signal, outcome) = (arg(args, 1).unwrap(), arg(args, 2).unwrap(), arg(args, 3).unwrap());
                if signal != "funding" {
                    return Err(rpc(
                        INVALID_PARAMETER,
                        "Only the funding signal is available in SPV mode",
                    ));
                }
                let outcome = match outcome {
                    "yes" => VoteOutcome::Yes,
                    "no" => VoteOutcome::No,
                    "abstain" => VoteOutcome::Abstain,
                    _ => {
                        return Err(rpc(
                            INVALID_PARAMETER,
                            "Invalid vote outcome. Please use one of the following: 'yes', 'no' or 'abstain'",
                        ));
                    }
                };
                let wallet = self.wallet.ok_or(ConsoleFailure::WalletRequired)?;
                let mns = s
                    .voting_masternodes(hash.to_string(), Some(wallet))
                    .await
                    .map_err(engine)?;
                let grant = self
                    .grant_id
                    .take()
                    .ok_or(ConsoleFailure::AuthorizationRequired {
                        purpose: GrantPurpose::Governance,
                        wallet: Some(wallet),
                    })?;
                let results = s
                    .cast_votes(
                        hash.to_string(),
                        outcome,
                        mns.iter().map(|m| m.pro_tx_hash.clone()).collect(),
                        grant,
                    )
                    .await
                    .map_err(engine)?;
                let ok = results.iter().filter(|r| r.failure.is_none()).count();
                let detail: Vec<(String, Json)> = results
                    .iter()
                    .map(|r| {
                        let mut f = vec![(
                            "result".to_string(),
                            Json::str(if r.failure.is_none() { "success" } else { "failed" }),
                        )];
                        if let Some(e) = &r.failure {
                            f.push(("errorMessage".into(), Json::str(e.to_string())));
                        }
                        (r.pro_tx_hash.clone(), Json::Obj(f))
                    })
                    .collect();
                Ok(Json::obj([
                    (
                        "overall",
                        Json::str(format!(
                            "Voted successfully {ok} time(s) and failed {} time(s).",
                            results.len() - ok
                        )),
                    ),
                    ("detail", Json::Obj(detail)),
                ]))
            }
            other => Err(ConsoleFailure::NotAvailable(format!("gobject {other}"))),
        }
    }
}

/// `(min quorum, maturity window)` of the session's network.
fn dw_governance_params(s: &NetworkSession) -> (u32, u32) {
    let p = s.governance_parameters();
    (p.min_quorum, p.maturity_window)
}

/// The fields of Core's `GetStateJson` the wallet knows, with the funding
/// tallies.
fn object_json(d: &dw_engine::governance::ProposalDetail) -> Json {
    Json::obj([
        ("DataHex", Json::str(hex::encode(d.raw_json.as_bytes()))),
        ("DataString", Json::str(d.raw_json.clone())),
        ("Hash", Json::str(d.row.hash.clone())),
        ("CollateralHash", Json::str(d.collateral_txid.clone())),
        ("ObjectType", Json::int(1u32)),
        ("CreationTime", Json::int(d.created_at)),
        ("AbsoluteYesCount", Json::int(i64::from(d.row.yes) - i64::from(d.row.no))),
        ("YesCount", Json::int(d.row.yes)),
        ("NoCount", Json::int(d.row.no)),
        ("AbstainCount", Json::int(d.row.abstain)),
    ])
}
