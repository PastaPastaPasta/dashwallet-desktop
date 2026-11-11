use dashcore::Txid;
use dashcore::hashes::Hash;
use dw_protx::shared::{dissolve::penalty_free_from, standby};

use super::dissolve::address_of;
use super::flow::loaded_all;
use super::standby::registration_of;
use super::store::{derive_stage, session_hash};
use crate::masternodes::MasternodesError;
use crate::masternodes::shared_records::{
    RewardDestination, ShareFact, SharedEligibility, SharedShares, SharedStage,
};
use crate::{NetworkSession, WalletId};

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
                    reward: RewardDestination {
                        at_registration: registration_session
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
                        latest_confirmed_local: None,
                        latest_attempt: None,
                    },
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
