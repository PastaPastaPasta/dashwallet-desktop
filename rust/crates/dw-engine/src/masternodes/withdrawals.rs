//! An evonode's Platform credit withdrawal on the facade (MG-13 P13,
//! mn-engine §2.5): the quote, the withdrawal and Try again under a
//! `MasternodeWithdraw` grant, the records, their recheck and the user's
//! marks. The engine side is `withdrawal/`. The quote reads once and returns
//! at once; while only Platform is behind (P4 (a)), its wait runs in the
//! background and sends §6's event when the destination is fixed
//! (`withdrawal_ready`), before any consent, so no grant waits on it.
//!
//! Each signing call takes its grant only after its own short wait for
//! P4's (a)–(c) (`withdraw`'s order); the grant is the user's consent to
//! the amount and, for a payout key, the destination it caps and binds
//! (P12). No call signs as operator or derives an operator key.

use std::time::Duration;

use dashcore::Network;
use dw_appdb::{WithdrawalRow, WithdrawalState};
use dw_vault::WithdrawalKey;

use super::withdrawal::{Destination, KeyView, Quote, WithdrawCall};
use super::withdrawal_records::{
    CreditWithdrawal, CreditWithdrawalState, HeldWithdrawalKey, NotExecutedFacts,
    WithdrawalCheckingReason, WithdrawalDestinationCheck, WithdrawalIdentityKey,
    WithdrawalKeyChoice, WithdrawalKeySource, WithdrawalMark, WithdrawalPlatformStatus,
    WithdrawalProofFacts, WithdrawalQuote, WithdrawalRules,
};
use super::{Masternodes, MasternodesError, display_hex, parse_pro_tx_hash};
use crate::platform::WithdrawAmount;
use crate::platform::withdrawal::{address_of, status_of};
use crate::{EngineError, WalletId};

impl Masternodes {
    /// What `wallet` can withdraw from the evonode's owner identity, and
    /// where to (MG-13 P13): its keys that match the identity's, the
    /// claimable credits and Drive's rules, each key's destination or why it
    /// is still `checking` (P4), Platform's status and the open record.
    /// Signs and spends nothing.
    pub async fn withdrawal_quote(
        &self,
        wallet: WalletId,
        pro_tx_hash: String,
    ) -> Result<WithdrawalQuote, MasternodesError> {
        let hash = parse_pro_tx_hash(&pro_tx_hash)?;
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            let quote = (s.mn_withdrawal_quote(wallet, &pro_tx_hash, Duration::ZERO)).await?;
            if behind(&quote) {
                s.mn_withdrawal_ready_wait(hash);
            }
            Ok(quote_of(
                &quote,
                display_hex(&hash),
                s.network.core_network(),
            ))
        })
        .await
    }

    /// Withdraws `amount` of the evonode identity's credits, signed by the
    /// wallet's `key` under a `MasternodeWithdraw` grant (P3, P12): an owner
    /// key takes no `destination` (Drive pays the payout address); a payout
    /// key pays `destination`, by default its own address. The record is
    /// stored before anything is sent, then the stored bytes are sent and
    /// read back (P5, P7). Returns the record, whatever the send's outcome.
    pub async fn withdraw_credits(
        &self,
        wallet: WalletId,
        pro_tx_hash: String,
        key: WithdrawalKeyChoice,
        amount: WithdrawAmount,
        destination: Option<String>,
        grant: String,
    ) -> Result<CreditWithdrawal, MasternodesError> {
        parse_pro_tx_hash(&pro_tx_hash)?;
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            let call = WithdrawCall {
                pro_tx_hash,
                key,
                amount,
                destination,
            };
            let row = s.mn_withdraw(wallet, &call, &grant, None).await?;
            s.masternodes.local_change();
            Ok(record_of(&row, s.network.core_network()))
        })
        .await
    }

    /// Try again (P15): a `refused` record's sibling at its nonce while Drive
    /// still takes it, by `key`, `amount` and `destination` as
    /// [`Self::withdraw_credits`] takes them, under a new grant. Returns the
    /// sibling; the refused record is `superseded` in the same write.
    pub async fn retry_withdrawal(
        &self,
        wallet: WalletId,
        id: String,
        key: WithdrawalKeyChoice,
        amount: WithdrawAmount,
        destination: Option<String>,
        grant: String,
    ) -> Result<CreditWithdrawal, MasternodesError> {
        check_id(&id)?;
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            let refused = s.mn_withdrawal(&id).await?;
            let call = WithdrawCall {
                pro_tx_hash: refused.signed.pro_tx_hash,
                key,
                amount,
                destination,
            };
            let row = (s.mn_withdraw(wallet, &call, &grant, Some(&id))).await?;
            s.masternodes.local_change();
            Ok(record_of(&row, s.network.core_network()))
        })
        .await
    }

    /// Every withdrawal record of the masternode, any wallet's, oldest
    /// first. Reads app.sqlite only.
    pub async fn withdrawals(
        &self,
        pro_tx_hash: String,
    ) -> Result<Vec<CreditWithdrawal>, MasternodesError> {
        parse_pro_tx_hash(&pro_tx_hash)?;
        self.protx_op(move |s| async move {
            let rows = s.mn_withdrawals(&pro_tx_hash).await?;
            let network = s.network.core_network();
            Ok(rows.iter().map(|r| record_of(r, network)).collect())
        })
        .await
    }

    /// Reads Platform again for the record's nonce group now (P8), as the
    /// background recheck does, and returns the record as it then stands.
    pub async fn recheck_withdrawal(
        &self,
        id: String,
    ) -> Result<CreditWithdrawal, MasternodesError> {
        check_id(&id)?;
        self.protx_op(move |s| async move {
            let before = s.mn_withdrawal(&id).await?;
            let row = s.mn_recheck_withdrawal(&id).await?;
            if row != before {
                s.masternodes.local_change();
            }
            Ok(record_of(&row, s.network.core_network()))
        })
        .await
    }

    /// The user's mark of a `couldnt_confirm` record (P9, MG-00 D47): kept
    /// apart from any proof; the record stays undecided and is still read.
    pub async fn mark_withdrawal(
        &self,
        id: String,
        mark: WithdrawalMark,
    ) -> Result<CreditWithdrawal, MasternodesError> {
        check_id(&id)?;
        self.protx_op(move |s| async move {
            let row = s.mn_mark_withdrawal(&id, mark.stored()).await?;
            s.masternodes.local_change();
            Ok(record_of(&row, s.network.core_network()))
        })
        .await
    }
}

/// A record id: 32 lower-case hex. The refusal doesn't repeat it.
fn check_id(id: &str) -> Result<(), EngineError> {
    if id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        Ok(())
    } else {
        Err(EngineError::InvalidArgument(
            "not a withdrawal id (32 lower-case hex)".into(),
        ))
    }
}

/// Whether a destination of the quote waits only for Platform (P4 (a)).
fn behind(quote: &Quote) -> bool {
    [&quote.owner_destination, &quote.payout_destination]
        .into_iter()
        .flatten()
        .any(|d| {
            matches!(
                d,
                Destination::Checking {
                    reason: WithdrawalCheckingReason::PlatformBehind
                }
            )
        })
}

fn held_of(k: &KeyView) -> HeldWithdrawalKey {
    let (kind, source) = match &k.key {
        WithdrawalKey::KeychainOwner { index } => (
            WithdrawalKeyChoice::Owner,
            WithdrawalKeySource::KeychainOwner { index: *index },
        ),
        WithdrawalKey::AttachedOwner { .. } => (
            WithdrawalKeyChoice::Owner,
            WithdrawalKeySource::AttachedOwner,
        ),
        WithdrawalKey::WalletPayout { path } => (
            WithdrawalKeyChoice::Payout,
            WithdrawalKeySource::WalletPayout {
                path: path.to_string(),
            },
        ),
        WithdrawalKey::AttachedPayout { .. } => (
            WithdrawalKeyChoice::Payout,
            WithdrawalKeySource::AttachedPayout,
        ),
    };
    HeldWithdrawalKey {
        kind,
        source,
        identity_key_id: k.identity_key,
    }
}

fn destination_of(d: &Destination) -> WithdrawalDestinationCheck {
    match d {
        Destination::Fixed {
            address,
            platform_height,
            core_height,
            header_tip,
            lag,
            ..
        } => WithdrawalDestinationCheck::Fixed {
            address: address.clone(),
            platform_height: *platform_height,
            core_height: *core_height,
            header_tip: *header_tip,
            lag: *lag,
        },
        Destination::Checking { reason } => {
            WithdrawalDestinationCheck::Checking { reason: *reason }
        }
    }
}

fn quote_of(q: &Quote, pro_tx_hash: String, network: Network) -> WithdrawalQuote {
    let s = &q.status;
    WithdrawalQuote {
        pro_tx_hash,
        owner_key: q.owner_key.as_ref().map(held_of),
        payout_keys: q.payout_keys.iter().map(held_of).collect(),
        claimable_credits: q.claimable,
        expected_duffs: q.expected_duffs,
        rules: WithdrawalRules {
            min_credits: q.rules.min,
            max_credits: q.rules.max,
            fee_credits: q.rules.fee,
        },
        owner_destination: q.owner_destination.as_ref().map(destination_of),
        payout_destination: q.payout_destination.as_ref().map(destination_of),
        status: WithdrawalPlatformStatus {
            identity_found: s.identity_found,
            keys: (s.keys.iter())
                .map(|(id, purpose, disabled)| WithdrawalIdentityKey {
                    id: *id,
                    purpose: purpose.to_string().to_lowercase(),
                    disabled: *disabled,
                })
                .collect(),
            balance: s.balance,
            platform_height: s.platform_height,
            core_height: s.core_height,
            header_tip: s.header_tip,
            lag: s.lag,
        },
        open: q.open.as_ref().map(|r| record_of(r, network)),
    }
}

/// A script's address on `network`, else its hex.
pub(super) fn address(script: &[u8], network: Network) -> String {
    address_of(script, network).unwrap_or_else(|| hex::encode(script))
}

fn state_of(state: WithdrawalState) -> CreditWithdrawalState {
    use CreditWithdrawalState as S;
    match state {
        WithdrawalState::Signed => S::Signed,
        WithdrawalState::Sent => S::Sent,
        WithdrawalState::Refused => S::Refused,
        WithdrawalState::Superseded => S::Superseded,
        WithdrawalState::CouldntConfirm => S::CouldntConfirm,
        WithdrawalState::MarkedReceived => S::MarkedReceived,
        WithdrawalState::MarkedNotReceived => S::MarkedNotReceived,
        WithdrawalState::Done => S::Done,
        WithdrawalState::SiblingPaid => S::SiblingPaid,
        WithdrawalState::NotExecuted => S::NotExecuted,
    }
}

fn record_of(r: &WithdrawalRow, network: Network) -> CreditWithdrawal {
    let s = &r.signed;
    CreditWithdrawal {
        id: s.id.clone(),
        pro_tx_hash: s.pro_tx_hash.clone(),
        identity: s.identity_id.clone(),
        created_by: s.created_by.clone(),
        transition_hash: s.transition_hash.clone(),
        key: match s.key_kind {
            dw_appdb::WithdrawalKeyKind::Owner => WithdrawalKeyChoice::Owner,
            dw_appdb::WithdrawalKeyKind::Payout => WithdrawalKeyChoice::Payout,
        },
        credits: s.amount,
        expected_duffs: s.expected_duffs,
        core_fee_per_byte: s.core_fee_per_byte,
        nonce: s.nonce,
        destination: s.script.as_deref().map(|b| address(b, network)),
        expected_destination: s.expected_script.as_deref().map(|b| address(b, network)),
        state: state_of(r.state),
        state_reason: r.state_reason.clone(),
        signed_at: s.signed_at,
        sent_at: r.sent_at,
        paid_to: r.paid_script.as_deref().map(|b| address(b, network)),
        document_status: r.document_status.map(status_of),
        proof: r.proof.as_ref().map(|p| WithdrawalProofFacts {
            document_id: p.document_id.clone(),
            platform_height: p.height,
            block_time: p.block_time,
        }),
        not_executed: r.evidence.as_ref().map(|e| NotExecutedFacts {
            nonce_height: e.nonce_height,
            identity_height: e.identity_height,
            document_height: e.document_height,
        }),
        last_checked: r.last_checked,
        mark: r.mark.map(|(m, _)| WithdrawalMark::of_stored(m)),
        marked_at: r.mark.map(|(_, at)| at),
        late_proof_link: r.late_proof_link.clone(),
    }
}
