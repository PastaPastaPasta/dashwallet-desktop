//! Masternode console commands (M3 R3): `masternodelist`, `protx list` /
//! `protx info`, `bls generate` / `bls fromsecret`, with Dash Core's result
//! shapes (`src/rpc/masternode.cpp` `masternodelist`, `src/rpc/evo.cpp`
//! `protx_list`, `protx_info`, `bls_generate`, `bls_fromsecret`) limited to
//! what an SPV wallet knows: a field the engine cannot know (PoSe score,
//! last paid, owner/payout/collateral of foreign masternodes) is left out,
//! never guessed. `masternodelist` keys entries by collateral outpoint as Core
//! does; a masternode whose collateral the wallets never saw is keyed by its
//! proTxHash instead. Provider transactions are not available from the
//! console (they need the wizard's operator-secret gate).

use dw_engine::masternodes::{
    MasternodeListStatus, MasternodeQuery, MasternodeRow, MasternodeType,
};

use super::*;

fn status_text(status: MasternodeListStatus) -> &'static str {
    match status {
        MasternodeListStatus::Active { .. } => "ENABLED",
        MasternodeListStatus::Banned { .. } => "POSE_BANNED",
        MasternodeListStatus::Retired => "RETIRED",
        MasternodeListStatus::Unknown => "UNKNOWN",
    }
}

fn type_text(t: MasternodeType) -> &'static str {
    match t {
        MasternodeType::Regular => "Regular",
        MasternodeType::Evo => "Evo",
    }
}

/// Core's `masternodelist json` entry, SPV fields only.
fn list_entry(r: &MasternodeRow) -> Json {
    let mut fields: Vec<(String, Json)> = vec![
        ("proTxHash".into(), Json::str(&r.pro_tx_hash)),
        ("type".into(), Json::str(type_text(r.node_type))),
        (
            "address".into(),
            r.service.as_deref().map_or(Json::Null, Json::str),
        ),
        ("status".into(), Json::str(status_text(r.status))),
        ("votingaddress".into(), Json::str(&r.voting_address)),
        ("pubkeyoperator".into(), Json::str(&r.operator_public_key)),
    ];
    if let Some(h) = r.registered_height {
        fields.push(("registeredheight".into(), Json::int(h)));
    }
    if let Some(a) = &r.owner_address {
        fields.push(("owneraddress".into(), Json::str(a)));
    }
    if let Some(p) = r.payout_addresses.first() {
        fields.push(("payee".into(), Json::str(p)));
    }
    if let Some(c) = &r.collateral_address {
        fields.push(("collateraladdress".into(), Json::str(c)));
    }
    if let Some(id) = &r.platform_node_id {
        fields.push(("platformNodeID".into(), Json::str(id)));
    }
    Json::Obj(fields)
}

/// The `masternodelist` key: Core's `txid-n` collateral, else the proTxHash.
fn list_key(r: &MasternodeRow) -> String {
    match r.collateral {
        Some(c) => format!("{}-{}", c.txid, c.vout),
        None => r.pro_tx_hash.clone(),
    }
}

/// Core's `protx info` object, SPV fields only.
fn protx_info(r: &MasternodeRow) -> Json {
    let mut state: Vec<(String, Json)> = vec![
        (
            "service".into(),
            r.service.as_deref().map_or(Json::Null, Json::str),
        ),
        ("votingAddress".into(), Json::str(&r.voting_address)),
        ("pubKeyOperator".into(), Json::str(&r.operator_public_key)),
    ];
    if let Some(h) = r.registered_height {
        state.push(("registeredHeight".into(), Json::int(h)));
    }
    if let Some(a) = &r.owner_address {
        state.push(("ownerAddress".into(), Json::str(a)));
    }
    if let Some(p) = r.payout_addresses.first() {
        state.push(("payoutAddress".into(), Json::str(p)));
    }
    if let Some(id) = &r.platform_node_id {
        state.push(("platformNodeID".into(), Json::str(id)));
    }
    let mut out: Vec<(String, Json)> = vec![
        ("type".into(), Json::str(type_text(r.node_type))),
        ("proTxHash".into(), Json::str(&r.pro_tx_hash)),
    ];
    if let Some(c) = &r.collateral {
        out.push(("collateralHash".into(), Json::str(c.txid.to_string())));
        out.push(("collateralIndex".into(), Json::int(c.vout)));
    }
    if let Some(reward) = &r.operator_reward {
        out.push((
            "operatorReward".into(),
            Json::Num(format!(
                "{}.{:02}",
                reward.percent_x100 / 100,
                reward.percent_x100 % 100
            )),
        ));
    }
    out.push(("state".into(), Json::Obj(state)));
    let mut wallet: Vec<(String, Json)> = Vec::new();
    for (key, role) in [
        ("hasOwnerKey", dw_engine::masternodes::OwnedRole::Owner),
        (
            "hasOperatorKey",
            dw_engine::masternodes::OwnedRole::Operator,
        ),
        ("hasVotingKey", dw_engine::masternodes::OwnedRole::Voting),
        (
            "ownsCollateral",
            dw_engine::masternodes::OwnedRole::Collateral,
        ),
        ("ownsPayeeScript", dw_engine::masternodes::OwnedRole::Payout),
        (
            "ownsOperatorRewardScript",
            dw_engine::masternodes::OwnedRole::OperatorPayout,
        ),
    ] {
        wallet.push((key.into(), Json::Bool(r.owned_roles.contains(&role))));
    }
    out.push(("wallet".into(), Json::Obj(wallet)));
    Json::Obj(out)
}

fn bls_pair(secret: &[u8; 32]) -> Result<Json, ConsoleFailure> {
    let (public, _) =
        dw_protx::bls::public_keys(secret).map_err(|e| rpc(code::INTERNAL_ERROR, e.to_string()))?;
    Ok(Json::obj([
        (
            "secret",
            Json::str(dw_protx::bls::secret_hex(secret).as_str()),
        ),
        ("public", Json::str(hex::encode(public))),
        ("scheme", Json::str("basic")),
    ]))
}

impl ConsoleContext {
    pub(super) async fn masternode_command(
        &mut self,
        method: &str,
        args: &[Zeroizing<String>],
    ) -> Result<Json, ConsoleFailure> {
        let s = Arc::clone(&self.session);
        match method {
            "bls" => match arg(args, 0) {
                Some("generate") => {
                    arity(method, args, 1, 2)?;
                    if arg(args, 1).map(parse_bool).transpose()? == Some(true) {
                        return Err(rpc(
                            code::INVALID_PARAMETER,
                            "legacy BLS scheme is deprecated",
                        ));
                    }
                    let secret = dw_protx::bls::generate()
                        .map_err(|e| rpc(code::INTERNAL_ERROR, e.to_string()))?;
                    bls_pair(&secret)
                }
                Some("fromsecret") => {
                    arity(method, args, 2, 3)?;
                    let secret = dw_protx::bls::parse_secret_hex(arg(args, 1).unwrap_or(""))
                        .map_err(|_| {
                            rpc(
                                code::INVALID_PARAMETER,
                                "Secret key must be a valid BLS secret key",
                            )
                        })?;
                    bls_pair(&secret)
                }
                _ => Err(usage(method)),
            },
            "masternodelist" => {
                arity(method, args, 0, 2)?;
                let mode = arg(args, 0).unwrap_or("json").to_lowercase();
                let filter = arg(args, 1).map(str::to_lowercase);
                match mode.as_str() {
                    "json" | "addr" | "status" | "pubkeyoperator" | "votingaddress"
                    | "owneraddress" | "payee" | "evo" => {}
                    "full" | "info" | "lastpaidblock" | "lastpaidtime" | "recent" => {
                        return Err(rpc(
                            code::INVALID_PARAMETER,
                            format!("mode {mode} needs a full node (PoSe and payment data)"),
                        ));
                    }
                    other => {
                        return Err(rpc(
                            code::INVALID_PARAMETER,
                            format!("Unknown mode {other}"),
                        ));
                    }
                }
                let rows = s.masternodes(MasternodeQuery::default()).await.rpc()?;
                let mut out: Vec<(String, Json)> = Vec::new();
                for r in &rows {
                    let value = match mode.as_str() {
                        "json" => list_entry(r),
                        "addr" => r.service.as_deref().map_or(Json::Null, Json::str),
                        "status" => Json::str(status_text(r.status)),
                        "pubkeyoperator" => Json::str(&r.operator_public_key),
                        "votingaddress" => Json::str(&r.voting_address),
                        "owneraddress" => match &r.owner_address {
                            Some(a) => Json::str(a),
                            None => continue,
                        },
                        "payee" => match r.payout_addresses.first() {
                            Some(p) => Json::str(p),
                            None => continue,
                        },
                        "evo" => {
                            if r.node_type != MasternodeType::Evo {
                                continue;
                            }
                            list_entry(r)
                        }
                        _ => unreachable!("modes are checked above"),
                    };
                    let key = list_key(r);
                    if let Some(f) = &filter
                        && !key.to_lowercase().contains(f)
                        && !value.write(0).to_lowercase().contains(f)
                    {
                        continue;
                    }
                    out.push((key, value));
                }
                Ok(Json::Obj(out))
            }
            "protx" => match arg(args, 0) {
                Some("list") => {
                    arity(method, args, 1, 3)?;
                    let kind = arg(args, 1).unwrap_or("registered");
                    let detailed = arg(args, 2).map(parse_bool).transpose()?.unwrap_or(false);
                    let rows = s.masternodes(MasternodeQuery::default()).await.rpc()?;
                    let keep = |r: &&MasternodeRow| match kind {
                        "registered" => !matches!(r.status, MasternodeListStatus::Retired),
                        "valid" => matches!(r.status, MasternodeListStatus::Active { .. }),
                        "wallet" => !r.owned_roles.is_empty(),
                        "evo" => r.node_type == MasternodeType::Evo,
                        _ => false,
                    };
                    if !matches!(kind, "registered" | "valid" | "wallet" | "evo") {
                        return Err(usage(method));
                    }
                    Ok(Json::Arr(
                        rows.iter()
                            .filter(keep)
                            .map(|r| {
                                if detailed {
                                    protx_info(r)
                                } else {
                                    Json::str(&r.pro_tx_hash)
                                }
                            })
                            .collect(),
                    ))
                }
                Some("info") => {
                    arity(method, args, 2, 3)?;
                    let hash = arg(args, 1).unwrap_or("").to_string();
                    let detail = s.masternode_detail(hash).await.map_err(|e| match e {
                        EngineError::Masternode(dw_engine::MasternodeFailure::NotFound(h)) => {
                            rpc(code::INVALID_PARAMETER, format!("{h} not found"))
                        }
                        EngineError::InvalidArgument(_) => rpc(
                            code::INVALID_PARAMETER,
                            "proTxHash must be hexadecimal string",
                        ),
                        other => engine_failure(other),
                    })?;
                    Ok(protx_info(&detail.row))
                }
                Some(_) => Err(rpc(
                    code::INVALID_PARAMETER,
                    "only protx list and protx info are available; provider transactions use \
                     the Masternodes tab (operator key confirmation)",
                )),
                None => Err(usage(method)),
            },
            other => Err(ConsoleFailure::NotAvailable(other.to_string())),
        }
    }
}
