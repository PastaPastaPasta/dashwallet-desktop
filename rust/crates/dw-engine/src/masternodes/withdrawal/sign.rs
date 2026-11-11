//! A withdrawal signed and sent (MG-13 P3, P5, P6, P10, P15), in DP6-02's
//! order (`platform/withdrawal.rs:725-746`): the bounded wait for (a)–(c)
//! outside the lock and before the grant; then, under the app-wide
//! identity lock, one final look without waiting, the open-record check,
//! the nonce, the grant's signature and the record's durable insert, all
//! before any network step; then the hand-off and P8 step 1's read-back.

use dashcore::Network;
use dpp::identity::accessors::IdentityGettersV0;
use dpp::identity::core_script::CoreScript;
use dpp::identity::identity_nonce::IDENTITY_NONCE_VALUE_FILTER;
use dpp::identity::identity_public_key::accessors::v0::IdentityPublicKeyGettersV0;
use dpp::identity::{Identity, Purpose};
use dpp::platform_value::BinaryData;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dpp::serialization::PlatformSerializable;
use dpp::state_transition::StateTransition;
use dpp::state_transition::identity_credit_withdrawal_transition::IdentityCreditWithdrawalTransition;
use dpp::state_transition::identity_credit_withdrawal_transition::v1::IdentityCreditWithdrawalTransitionV1;
use dpp::withdrawal::Pooling;
use dw_appdb::{SignedWithdrawal, WithdrawalKeyKind, WithdrawalRow, WithdrawalState};
use dw_vault::WithdrawalKey;

use super::check::{at_least, held, identity_of, look, look_until};
use super::drive::window;
use super::net::{Answer, MnWithdrawNet};
use super::recheck::read_back;
use super::{
    Alarm, Authorize, Env, WalletKeys, WithdrawalError, fault_after, fault_before, identity_lock,
    load, on_db, points,
};
use crate::platform::WithdrawAmount;
use crate::platform::withdrawal::{CORE_FEE_PER_BYTE, Rules, expected_duffs, takes};

/// Which kind of key signs (P2, D23): the owner key (Drive pays its
/// window's script; no destination may be given) or a payout key (the
/// approved script: `destination`, else the signing key's address).
pub(crate) use crate::masternodes::WithdrawalKeyChoice;

/// `withdraw_credits`' arguments (P13).
#[derive(Debug, Clone)]
pub(crate) struct Request {
    /// Wire order.
    pub(crate) pro_tx_hash: [u8; 32],
    pub(crate) key: WithdrawalKeyChoice,
    pub(crate) amount: WithdrawAmount,
    /// A payout-key withdrawal's destination (`destination()`'s script);
    /// `None` pays the signing key's address. An owner-key one takes none.
    pub(crate) destination: Option<CoreScript>,
    /// The wallet, as a label (P5).
    pub(crate) created_by: String,
}

/// The identity in the records' encoding.
pub(crate) fn shown(id: &Identifier) -> String {
    id.to_string(Encoding::Base58)
}

/// `withdraw_credits` (module doc): the record, once inserted and handed
/// off. A refusal before the insert signed nothing.
pub(crate) async fn withdraw(
    env: &impl Env,
    net: &impl MnWithdrawNet,
    keys: &impl WalletKeys,
    auth: impl Authorize,
    network: Network,
    req: &Request,
) -> Result<WithdrawalRow, WithdrawalError> {
    signed_and_sent(env, net, keys, auth, network, req, None).await
}

/// Try again (P15): from `refused` while the record's nonce is still taken,
/// a sibling at that nonce (amount, key and destination may change; P2–P4
/// apply again), inserted in the one transaction that supersedes the
/// refused record.
pub(crate) async fn retry(
    env: &impl Env,
    net: &impl MnWithdrawNet,
    keys: &impl WalletKeys,
    auth: impl Authorize,
    network: Network,
    refused_id: &str,
    req: &Request,
) -> Result<WithdrawalRow, WithdrawalError> {
    let refused = load(env, refused_id).await?;
    if refused.signed.pro_tx_hash != crate::masternodes::display_hex(&req.pro_tx_hash) {
        return Err(WithdrawalError::TargetMismatch);
    }
    if refused.state != WithdrawalState::Refused {
        return Err(WithdrawalError::RetryState);
    }
    signed_and_sent(env, net, keys, auth, network, req, Some(refused_id)).await
}

async fn signed_and_sent(
    env: &impl Env,
    net: &impl MnWithdrawNet,
    keys: &impl WalletKeys,
    auth: impl Authorize,
    network: Network,
    req: &Request,
    sibling_of: Option<&str>,
) -> Result<WithdrawalRow, WithdrawalError> {
    if req.key == WithdrawalKeyChoice::Owner && req.destination.is_some() {
        return Err(crate::EngineError::InvalidArgument(
            "an owner-key withdrawal pays the payout address: it takes no destination".into(),
        )
        .into());
    }
    // P4: the short wait, outside the lock and before the grant.
    look_until(env, net, &req.pro_tx_hash, env.waits().sign)
        .await?
        .holds()?;
    let id = identity_of(&req.pro_tx_hash);
    let lock = identity_lock(id);
    let guard = lock.lock().await;
    // The final look, without waiting (P4, r4 n1): a miss is
    // `platform_behind` (or its other reason) and nothing is signed.
    let seen = look(env, net, &req.pro_tx_hash).await?;
    seen.holds()?;
    let identity = seen.identity()?.clone();
    let records = group_records(env, &id).await?;
    match sibling_of {
        None if records.iter().any(|r| r.state.is_open()) => return Err(WithdrawalError::Pending),
        // Still refused under the lock, or the grant would be redeemed for
        // an insert the supersede refuses.
        Some(refused)
            if !(records.iter())
                .any(|r| r.signed.id == refused && r.state == WithdrawalState::Refused) =>
        {
            return Err(WithdrawalError::RetryState);
        }
        _ => {}
    }
    let plan = plan(net, keys, network, &identity, req).await?;
    let (nonce, floor) = nonce_for(env, net, &id, &records, sibling_of).await?;
    // P5: the final look's identity read is used for the record too.
    let floor = match &seen.identity.read {
        r if r.verified => floor.max(r.height),
        _ => floor,
    };
    let signed = sign(env, auth, req, &identity, &plan, nonce).await?;
    let inserted = signed.clone();
    let refused = sibling_of.map(str::to_owned);
    let applied = on_db(env, move |db| match refused {
        None => db
            .insert_withdrawal_durable(&inserted, floor)
            .map(|()| true),
        Some(refused) => db.supersede_and_insert_withdrawal_durable(&refused, &inserted, floor),
    })
    .await?;
    if !applied {
        // The refused record moved.
        return Err(WithdrawalError::RetryState);
    }
    drop(guard);
    let row = load(env, &signed.id).await?;
    hand_off(env, net, &row, points::BROADCAST).await?;
    // P8 step 1 only: a match decides; nothing else does here.
    if let Err(e) = read_back(env, net, &id, row.signed.nonce).await {
        tracing::info!(error = %e, "reading the masternode withdrawal back");
    }
    load(env, &signed.id).await
}

/// The identity's records, every group.
pub(crate) async fn group_records(
    env: &impl Env,
    id: &Identifier,
) -> Result<Vec<WithdrawalRow>, WithdrawalError> {
    let shown = shown(id);
    on_db(env, move |db| db.withdrawals_of_identity(&shown)).await
}

/// What the withdrawal signs, decided against the final look's identity.
struct Plan {
    key: WithdrawalKey,
    /// The identity key it signs with.
    identity_key: dpp::identity::IdentityPublicKey,
    /// The signed `output_script` (payout key); `None` for the owner key.
    script: Option<CoreScript>,
    /// The owner key's destination as this look saw it.
    expected: Option<CoreScript>,
    credits: u64,
    /// Drive's fee reserve the grant's charge adds (P11).
    fee: u64,
    expected_duffs: u64,
}

async fn plan(
    net: &impl MnWithdrawNet,
    keys: &impl WalletKeys,
    network: Network,
    identity: &Identity,
    req: &Request,
) -> Result<Plan, WithdrawalError> {
    let (owner, payout) = held(keys, identity).await?;
    let (view, purpose, script, expected) = match req.key {
        WithdrawalKeyChoice::Owner => {
            let view = owner.ok_or(WithdrawalError::KeyMissing)?;
            // Drive's window must pay somewhere, or Drive refuses (F3).
            let window = window(identity);
            let paid = window.script()?.clone();
            (view, Purpose::OWNER, None, Some(paid))
        }
        WithdrawalKeyChoice::Payout => {
            let view = payout
                .into_iter()
                .next()
                .ok_or(WithdrawalError::KeyMissing)?;
            let own = (identity.public_keys().get(&view.identity_key))
                .and_then(super::drive::script_of)
                .ok_or(WithdrawalError::KeyMissing)?;
            let script = req.destination.clone().unwrap_or(own);
            check_script(&script, network)?;
            (view, Purpose::TRANSFER, Some(script), None)
        }
    };
    let identity_key = (identity.public_keys().get(&view.identity_key))
        .filter(|k| k.purpose() == purpose)
        .cloned()
        .ok_or(WithdrawalError::KeyMissing)?;
    // Under the lock, from the final look's proved balance (P11).
    let balance = identity.balance();
    let rules = Rules::at(net.version())?;
    let credits = rules.credits(balance, req.amount)?;
    Ok(Plan {
        key: view.key,
        identity_key,
        script,
        expected,
        credits,
        fee: rules.fee,
        expected_duffs: expected_duffs(credits, net.version())?,
    })
}

/// A payout-key destination is P2PKH or P2SH, the kinds Core takes (D23).
fn check_script(script: &CoreScript, network: Network) -> Result<(), WithdrawalError> {
    let s = dashcore::ScriptBuf::from_bytes(script.as_bytes().to_vec());
    if (s.is_p2pkh() || s.is_p2sh()) && dashcore::Address::from_script(&s, network).is_ok() {
        Ok(())
    } else {
        Err(crate::EngineError::InvalidArgument(
            "only P2PKH and P2SH addresses take withdrawals".into(),
        )
        .into())
    }
}

/// P10 over the identity's nonce groups, from a proved nonce read at or
/// above every floor of its records (never the SDK's cache) that passed
/// rule 5 (P12):
/// - **same nonce**: an undecided group whose nonce `n` Drive still takes
///   signs at `n` (Try again, or after a mark);
/// - **tip + 1**: only when every group's `n`, final or not, is at or below
///   the tip and not taken;
/// - a final group whose `n` is still taken is an invariant violation:
///   logged, alarmed, refused (`withdrawal_pending`);
/// - **neither** (an undecided group above a stale tip, say):
///   `withdrawal_pending`.
///
/// For Try again (`sibling_of`), only the refused record's own nonce, while
/// taken; else `withdrawal_retry_state` (P15).
///
/// Returns the nonce and the new record's starting floor: the read's height
/// when SPV-verified, else 0 (only SPV-provenance reads raise a floor, P5).
async fn nonce_for(
    env: &impl Env,
    net: &impl MnWithdrawNet,
    id: &Identifier,
    records: &[WithdrawalRow],
    sibling_of: Option<&str>,
) -> Result<(u64, u64), WithdrawalError> {
    let least = floor_of(records);
    let read = at_least(env, least, || net.nonce_state(*id)).await?;
    if read.read.height < least {
        return Err(WithdrawalError::Pending);
    }
    if !read.passed {
        return Err(WithdrawalError::Unverified);
    }
    let floor = if read.read.verified {
        read.read.height
    } else {
        0
    };
    let state = read.read.value;
    let tip = state & IDENTITY_NONCE_VALUE_FILTER;
    let mut groups: Vec<(u64, bool)> = Vec::new();
    for r in records {
        match groups.iter_mut().find(|(n, _)| *n == r.signed.nonce) {
            Some((_, undecided)) => *undecided |= !r.state.is_final(),
            None => groups.push((r.signed.nonce, !r.state.is_final())),
        }
    }
    if let Some(refused) = sibling_of {
        let n = (records.iter().find(|r| r.signed.id == refused))
            .map(|r| r.signed.nonce)
            .ok_or(WithdrawalError::NotFound)?;
        if !takes(state, n, *id) {
            return Err(WithdrawalError::RetryState);
        }
        return Ok((n, floor));
    }
    for &(n, undecided) in &groups {
        if !undecided && takes(state, n, *id) {
            env.alarm(Alarm {
                identity: shown(id),
                nonce: n,
                what: "a final group's nonce is still taken by Drive".into(),
            });
            return Err(WithdrawalError::Pending);
        }
    }
    let takeable: Vec<u64> = (groups.iter())
        .filter(|(n, undecided)| *undecided && takes(state, *n, *id))
        .map(|(n, _)| *n)
        .collect();
    match takeable.as_slice() {
        [n] => return Ok((*n, floor)),
        [] => {}
        _ => return Err(WithdrawalError::Pending),
    }
    if groups
        .iter()
        .all(|(n, _)| *n <= tip && !takes(state, *n, *id))
    {
        return Ok((tip + 1, floor));
    }
    Err(WithdrawalError::Pending)
}

/// The highest floor of `records` (P5): every read deciding on them is at
/// or above it.
pub(crate) fn floor_of(records: &[WithdrawalRow]) -> u64 {
    records.iter().map(|r| r.floor_height).max().unwrap_or(0)
}

/// Builds the V1 transition (P3: no user fee increase, Platform's minimum
/// Core fee, pooling `Never`; the owner key with no output script, the
/// payout key with the approved one), signs it at `nonce` through the
/// grant, and returns the record to insert.
async fn sign(
    env: &impl Env,
    auth: impl Authorize,
    req: &Request,
    identity: &Identity,
    plan: &Plan,
    nonce: u64,
) -> Result<SignedWithdrawal, WithdrawalError> {
    let id = identity.id();
    let mut v1 = IdentityCreditWithdrawalTransitionV1 {
        identity_id: id,
        amount: plan.credits,
        core_fee_per_byte: CORE_FEE_PER_BYTE,
        pooling: Pooling::Never,
        output_script: plan.script.clone(),
        nonce,
        user_fee_increase: 0,
        signature_public_key_id: plan.identity_key.id(),
        signature: BinaryData::default(),
    };
    let v1_transition = |v1: IdentityCreditWithdrawalTransitionV1| {
        StateTransition::IdentityCreditWithdrawal(IdentityCreditWithdrawalTransition::V1(v1))
    };
    fault_before(env.db(), points::SIGN)?;
    let charge = plan.credits.saturating_add(plan.fee);
    let signature = auth
        .sign(
            charge,
            &plan.key,
            &plan.identity_key,
            &v1_transition(v1.clone()),
            req.pro_tx_hash,
        )
        .await?;
    v1.signature = BinaryData::new(signature.to_vec());
    let transition = v1_transition(v1);
    let bytes = transition.serialize_to_bytes().map_err(internal)?;
    let hash = transition.transaction_id().map_err(internal)?;
    fault_after(env.db(), points::SIGN)?;
    let mut row_id = [0u8; 16];
    rand::Rng::fill(&mut rand::thread_rng(), &mut row_id);
    Ok(SignedWithdrawal {
        id: hex::encode(row_id),
        transition_hash: hex::encode(hash),
        created_by: req.created_by.clone(),
        pro_tx_hash: crate::masternodes::display_hex(&req.pro_tx_hash),
        identity_id: shown(&id),
        key_kind: if plan.key.is_owner() {
            WithdrawalKeyKind::Owner
        } else {
            WithdrawalKeyKind::Payout
        },
        amount: plan.credits,
        core_fee_per_byte: CORE_FEE_PER_BYTE,
        pooling: Pooling::Never as u8,
        expected_duffs: plan.expected_duffs,
        nonce,
        signed_bytes: bytes,
        script: plan.script.as_ref().map(|s| s.as_bytes().to_vec()),
        expected_script: plan.expected.as_ref().map(|s| s.as_bytes().to_vec()),
        signed_at: env.now(),
    })
}

fn internal(e: impl std::fmt::Display) -> WithdrawalError {
    crate::platform::PlatformError::Internal {
        detail: format!("the masternode withdrawal: {e}"),
    }
    .into()
}

/// One hand-off of `row`'s stored bytes, unchanged (P5, P7): `sent_at`
/// durably first, then the broadcast between `point`'s fault stages, then
/// its answer recorded: a merits refusal is `refused` with its reason
/// (noted "after an earlier send" when an attempt had already set
/// `sent_at`); every other answer moves a `signed` record to `sent`. A
/// record no longer in the state read is left alone (compare-and-set).
pub(crate) async fn hand_off(
    env: &impl Env,
    net: &impl MnWithdrawNet,
    row: &WithdrawalRow,
    point: &'static str,
) -> Result<Answer, WithdrawalError> {
    let (id, now) = (row.signed.id.clone(), env.now());
    let prior = on_db(env, move |db| db.begin_withdrawal_send_durable(&id, now))
        .await?
        .ok_or(WithdrawalError::NotFound)?;
    fault_before(env.db(), point)?;
    let answer = net.broadcast(&row.signed.signed_bytes).await;
    fault_after(env.db(), point)?;
    use WithdrawalState::*;
    let (from, to, reason): (&[WithdrawalState], _, _) = match &answer {
        Answer::Refused(reason) => {
            let reason = if prior {
                format!("{reason} (after an earlier send)")
            } else {
                reason.clone()
            };
            (&[Signed, Sent], Refused, Some(reason))
        }
        _ => (&[Signed], Sent, None),
    };
    let id = row.signed.id.clone();
    on_db(env, move |db| {
        db.set_withdrawal_state_durable(&id, from, to, reason.as_deref())
    })
    .await?;
    tracing::info!(answer = ?answer, "a masternode withdrawal handed off");
    Ok(answer)
}
