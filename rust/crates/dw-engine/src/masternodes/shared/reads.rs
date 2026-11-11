use dashcore::Txid;
use dashcore::hashes::Hash;
use dw_appdb::MasternodeOpRow;
use dw_protx::shared::{dissolve::penalty_free_from, standby};

use super::dissolve::address_of;
use super::flow::loaded_all;
use super::standby::registration_of;
use super::store::{ChainView, derive_stage, session_hash};
use crate::masternodes::protx::ops;
use crate::masternodes::protx_records::{ProTxOperationState, ProTxTerms};
use crate::masternodes::shared_records::{
    RewardAttempt, RewardAttemptState, RewardDestination, RewardUpdate, ShareFact,
    SharedEligibility, SharedShares, SharedStage, UpdateShareRequest,
};
use crate::masternodes::{MasternodesError, display_hex};
use crate::{EngineError, NetworkSession, WalletId};

fn reward_destination(
    rows: &[MasternodeOpRow],
    chain: &ChainView,
    hash: &str,
    index: u16,
    at_registration: String,
) -> Result<RewardDestination, MasternodesError> {
    let mut updates = Vec::new();
    for row in rows
        .iter()
        .filter(|r| r.kind == "update_share" && r.pro_tx_hash.as_deref() == Some(hash))
    {
        let request: UpdateShareRequest = serde_json::from_str(&row.request)
            .map_err(|e| EngineError::Internal(format!("stored share update: {e}")))?;
        if request.share_index != index {
            continue;
        }
        let terms: ProTxTerms = serde_json::from_str(&row.terms)
            .map_err(|e| EngineError::Internal(format!("stored ProTx terms: {e}")))?;
        let state = ops::state_of(row, &chain.history, &chain.unspent);
        updates.push((row, terms.payout_address, state));
    }
    let confirmed = updates
        .iter()
        .filter_map(|(row, address, state)| {
            if let ProTxOperationState::Confirmed { height } = state {
                Some((*height, row.created_at, &row.id, address))
            } else {
                None
            }
        })
        .max_by_key(|(height, at, id, _)| (*height, *at, *id));
    let latest_confirmed_local = confirmed.map(|(height, _, id, address)| RewardUpdate {
        operation: id.clone(),
        address: address.clone(),
        confirmed_height: height,
    });
    let latest_attempt = updates
        .iter()
        .filter(|(row, _, state)| {
            !matches!(state, ProTxOperationState::Confirmed { .. })
                && confirmed.is_none_or(|(_, at, _, _)| row.created_at > at)
        })
        .filter_map(|(row, address, state)| {
            let state = match state {
                ProTxOperationState::Signed => RewardAttemptState::Signed,
                ProTxOperationState::NotSent => RewardAttemptState::NotSent,
                ProTxOperationState::Sent => RewardAttemptState::Sent,
                ProTxOperationState::Conflicted => RewardAttemptState::Conflicted,
                _ => return None,
            };
            Some((row, address, state))
        })
        .max_by_key(|(row, _, _)| (row.created_at, &row.id))
        .map(|(row, address, state)| RewardAttempt {
            operation: row.id.clone(),
            address: address.clone(),
            state,
            sent_at: row.sent_at,
        });
    Ok(RewardDestination {
        at_registration,
        latest_confirmed_local,
        latest_attempt,
    })
}

impl NetworkSession {
    pub(super) async fn shared_share_facts(
        &self,
        wallet: WalletId,
        hash: [u8; 32],
    ) -> Result<SharedShares, MasternodesError> {
        let chain = self.shared_chain(wallet).await?;
        let reg = registration_of(&chain, hash)?;
        let mine = self.my_shares(wallet, &reg.shares).await?;
        let active = self.v24_active().await?;
        let ctx = match self.protx_context(wallet).await {
            Ok(ctx) => Some(ctx),
            Err(MasternodesError::ListUnavailable) => None,
            Err(e) => return Err(e),
        };
        let list_height = ctx.as_ref().and_then(|ctx| ctx.list_height);
        let tip = self.hub.tracker().tip_height().unwrap_or(0);
        let spend_height = list_height.unwrap_or(0).max(tip).saturating_add(1);
        let eligibility = if !active {
            SharedEligibility::UpgradeUnknown
        } else if let Some(ctx) = &ctx {
            if !ctx
                .list_height
                .is_some_and(|height| height > reg.registered_height)
            {
                SharedEligibility::TooEarly
            } else if !ctx.entries.iter().any(|entry| entry.pro_tx_hash == hash) {
                SharedEligibility::Delisted
            } else {
                self.listed_of(ctx, &reg)?;
                SharedEligibility::Ok
            }
        } else {
            SharedEligibility::ListUnavailable
        };
        let registration_session =
            loaded_all(self.shared_rows(wallet).await?, self.network.core_network())
                .into_iter()
                .find(|loaded| {
                    matches!(derive_stage(loaded, &chain), SharedStage::Confirmed { .. })
                        && session_hash(loaded, &chain) == Some(Txid::from_byte_array(hash))
                });
        let session = registration_session
            .as_ref()
            .map(|loaded| loaded.row.id.clone());
        let w = wallet.to_string();
        let rows = self.appdb_op(move |db| db.masternode_ops(&w)).await?;
        let shown = display_hex(&hash);
        let shares = reg
            .shares
            .iter()
            .enumerate()
            .map(|(index, share)| {
                Ok(ShareFact {
                    index: index as u16,
                    amount: share.amount,
                    refund_address: address_of(&share.refund_script, self.network.core_network())
                        .unwrap_or_default(),
                    mine: mine.iter().any(|(i, _)| usize::from(*i) == index),
                    reward: reward_destination(
                        &rows,
                        &chain,
                        &shown,
                        index as u16,
                        registration_session
                            .as_ref()
                            .and_then(|loaded| loaded.session.shares.get(index))
                            .map(|share| {
                                if share.reward_address.is_empty() {
                                    share.refund_address.clone()
                                } else {
                                    share.reward_address.clone()
                                }
                            })
                            .unwrap_or_else(|| {
                                address_of(
                                    share.effective_reward_script(),
                                    self.network.core_network(),
                                )
                                .unwrap_or_default()
                            }),
                    )?,
                })
            })
            .collect::<Result<Vec<_>, MasternodesError>>()?;
        let mn = reg.mn();
        Ok(SharedShares {
            registered_height: reg.registered_height,
            spend_height,
            early_period_blocks: reg.early_period_blocks,
            early_penalty: standby::early_penalty(&mn),
            full_valid_from: penalty_free_from(&mn),
            eligibility,
            full_valid_now: standby::full_valid(&mn, spend_height),
            session,
            shares,
        })
    }
}
