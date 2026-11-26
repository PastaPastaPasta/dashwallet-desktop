//! Credits from an identity to the wallet's own Platform address (ROUTE-22;
//! design `docs/design/C-routes.md` D-C6): an
//! IdentityCreditTransferToAddresses with one recipient and its quote,
//! behind the facade calls of `credits.rs`. Not part of the facade.
//!
//! - **The rules (C6-5).** The reserve is the larger of dpp's minimum fee
//!   for one recipient and a bound on the fee Platform meters
//!   (`credit_costs::transfer_to_address_metered_bound`). "Max" is the
//!   balance less the reserve; an amount below the version's
//!   `min_output_amount` is out of range.
//! - **The write.** As a withdrawal (`withdrawal.rs`): the money gate, the
//!   identity's write lock, the `Withdraw` lease, a CRITICAL TRANSFER key,
//!   dpp's own transition at a nonce the engine chooses. The nonce is the
//!   withdrawals' (C6-6, [`WithdrawalNonces`]): of an unconfirmed write and
//!   the next, at most one executes, and no second transfer signs while one
//!   may still execute.
//! - **The outcome (C6-7).** Once signed, the transfer counts as executed
//!   only by the identity's proved nonce ([`executed`]), never by the
//!   answer or a balance.

use std::collections::BTreeMap;
use std::sync::Arc;

use dpp::address_funds::PlatformAddress;
use dpp::dashcore::Network;
use dpp::identity::accessors::IdentityGettersV0;
use dpp::identity::signer::Signer;
use dpp::identity::{Identity, IdentityPublicKey};
use dpp::prelude::Identifier;
use dpp::state_transition::identity_credit_transfer_to_addresses_transition::IdentityCreditTransferToAddressesTransition;
use dpp::state_transition::identity_credit_transfer_to_addresses_transition::methods::IdentityCreditTransferToAddressesTransitionMethodsV0;
use dpp::state_transition::identity_credit_transfer_to_addresses_transition::v0::IdentityCreditTransferToAddressesTransitionV0;
use dpp::state_transition::{StateTransition, StateTransitionEstimatedFeeValidation};
use dpp::version::PlatformVersion;
use platform_wallet::PlatformWallet;

use super::credit_costs::transfer_to_address_metered_bound;
use super::credits::{WithdrawAmount, WithdrawToPlatformOutcome, WithdrawToPlatformQuote};
use super::errors::{CreditsError, IdentityError, PlatformError};
use super::flows::{FlowKind, GrantRequest};
use super::identity::KeyPurpose;
use super::identity_writes::identity_locks;
use super::money_gate::Subject;
use super::platform_addresses::RecipientProblem;
use super::platform_moves::{
    address_sends_open, bech32m, next_receive_address, parse, pool, recipient, sends_active,
};
use super::registration_flow::parse_identity;
use super::requests::{flow_signer, leased, write_lease, write_permit};
use super::withdrawal::{
    READ_BACK_TRIES, Signed, Unsent, WithdrawNet, WithdrawalNonces, executed, internal,
    landed_error, nonce_for, own, platform,
};
use crate::{NetworkSession, WalletId};

/// A transfer's rules at one protocol version (module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TransferRules {
    /// The least a transfer pays, in credits.
    pub(crate) min: u64,
    /// The fee reserve, in credits.
    pub(crate) fee: u64,
}

impl TransferRules {
    /// The rules at `version`; below address funds
    /// `platform.feature_off{platform_addresses}`.
    pub(crate) fn at(version: &PlatformVersion) -> Result<Self, PlatformError> {
        sends_active(version)?;
        let one = IdentityCreditTransferToAddressesTransitionV0 {
            recipient_addresses: BTreeMap::from([(PlatformAddress::P2pkh([0; 20]), 0)]),
            ..Default::default()
        };
        let min_fee = IdentityCreditTransferToAddressesTransition::V0(one)
            .calculate_min_required_fee(version)
            .map_err(internal)?;
        Ok(Self {
            min: version
                .dpp
                .state_transitions
                .address_funds
                .min_output_amount,
            fee: min_fee.max(transfer_to_address_metered_bound(version)),
        })
    }

    /// The credits `amount` takes from an identity holding `balance`:
    /// "Max" the balance less the reserve, one below [`Self::min`]
    /// `platform.insufficient_credits`; an amount below the least
    /// `credits.withdrawal_out_of_range{min, max}`, `max` the balance less
    /// the reserve, and above that `platform.insufficient_credits`.
    pub(crate) fn credits(
        &self,
        balance: u64,
        amount: WithdrawAmount,
    ) -> Result<u64, CreditsError> {
        let max = balance.saturating_sub(self.fee);
        let insufficient = |credits: u64| PlatformError::InsufficientCredits {
            needed: credits.saturating_add(self.fee),
            available: balance,
        };
        match amount {
            WithdrawAmount::All if max < self.min => Err(insufficient(self.min).into()),
            WithdrawAmount::All => Ok(max),
            WithdrawAmount::Credits { credits } if credits < self.min => {
                Err(CreditsError::WithdrawalOutOfRange { min: self.min, max })
            }
            WithdrawAmount::Credits { credits } if credits > max => {
                Err(insufficient(credits).into())
            }
            WithdrawAmount::Credits { credits } => Ok(credits),
        }
    }
}

/// The credits taken, the rules they were taken by and the identity's
/// proved balance, read before the version (it ratchets the SDK).
async fn plan(
    net: &impl WithdrawNet,
    id: Identifier,
    amount: WithdrawAmount,
) -> Result<(u64, TransferRules), CreditsError> {
    let balance =
        (net.balance(id).await?).ok_or(PlatformError::Identity(IdentityError::NotFound))?;
    let rules = TransferRules::at(net.version())?;
    Ok((rules.credits(balance, amount)?, rules))
}

/// `to` as one of the wallet's own Platform addresses (C6-1); anything
/// else is `invalid_argument`.
async fn destination(
    wallet: &PlatformWallet,
    network: Network,
    to: &str,
) -> Result<PlatformAddress, PlatformError> {
    let pool = pool(wallet).await?;
    match (recipient(network, to, &pool), parse(to.trim())) {
        (Err(RecipientProblem::Own), Some(address)) => Ok(address),
        _ => Err(PlatformError::InvalidArgument {
            detail: "credits move only to this wallet's own Platform addresses".into(),
        }),
    }
}

/// `DashPay::withdraw_to_platform_quote`.
pub(crate) async fn quote(
    net: &impl WithdrawNet,
    session: &NetworkSession,
    wallet: &PlatformWallet,
    network: Network,
    identity: &str,
    amount: WithdrawAmount,
) -> Result<WithdrawToPlatformQuote, CreditsError> {
    address_sends_open(network)?;
    let id = parse_identity(identity)?;
    own(wallet, id).await?;
    let (credits, rules) = plan(net, id, amount).await?;
    Ok(WithdrawToPlatformQuote {
        credits,
        fee_credits: rules.fee,
        to: next_receive_address(session, wallet, network)
            .await?
            .address,
        grant: GrantRequest {
            max_duffs: 0,
            max_credits: credits + rules.fee,
        },
    })
}

/// The signed transfer of `credits` to `to`: dpp's own construction with
/// `key`, at `nonce`, no fee increase.
pub(crate) async fn transition(
    identity: &Identity,
    (to, credits, nonce): (PlatformAddress, u64, u64),
    key: &IdentityPublicKey,
    signer: &impl Signer<IdentityPublicKey>,
    version: &PlatformVersion,
) -> Result<StateTransition, PlatformError> {
    IdentityCreditTransferToAddressesTransition::try_from_identity(
        identity,
        BTreeMap::from([(to, credits)]),
        0,
        signer,
        Some(key),
        nonce,
        version,
        None,
    )
    .await
    .map_err(internal)
}

/// C6-7 after the broadcast: the nonce read again, at least as recent as
/// `height`, until it decides, at most [`READ_BACK_TRIES`] times; a failed
/// read ends it, and decides nothing.
async fn decide(
    net: &impl WithdrawNet,
    signed: &[Signed],
    (id, k): (Identifier, u64),
    height: u64,
) -> Result<Option<Signed>, PlatformError> {
    for _ in 0..READ_BACK_TRIES {
        let (state, at) = net.nonce_state(id).await?;
        if at < height {
            continue;
        }
        if let Some(s) = executed(net, signed, (id, k), (state, at)).await? {
            return Ok(Some(s));
        }
    }
    Ok(None)
}

/// `DashPay::withdraw_to_platform` (module doc).
pub(crate) async fn transfer(
    net: &impl WithdrawNet,
    session: &Arc<NetworkSession>,
    wallet: &PlatformWallet,
    (wallet_id, identity): (WalletId, &str),
    (to, amount): (&str, WithdrawAmount),
    grant: &str,
) -> Result<WithdrawToPlatformOutcome, CreditsError> {
    let network = session.vault.network();
    address_sends_open(network)?;
    let id = parse_identity(identity)?;
    let address = destination(wallet, network, to).await?;
    own(wallet, id).await?;
    (session.money_gate(Subject::Identity(wallet_id, id)).await)
        .map_err(|refusal| CreditsError::from(refusal.platform()))?;
    let _refresh = session.after_spend(wallet_id, id);
    let locks = identity_locks(wallet_id, id);
    let _write = locks.write.lock().await;
    let (index, me) = own(wallet, id).await?;
    let (credits, rules) = plan(net, id, amount).await?;
    let version = net.version();
    let quote = credits + rules.fee;
    let nonces: &WithdrawalNonces = &session.withdrawal_nonces;
    let (nonce, height) = nonce_for(net, nonces, id, true).await?;
    let lease = write_lease(
        session,
        wallet_id,
        grant,
        (&[FlowKind::Withdraw], quote),
        (index, &me),
        || Ok(()),
    )
    .await
    .map_err(platform)?;
    let permit = write_permit(&lease, "withdraw_to_platform", quote)?;
    let signer = flow_signer(&permit, index, &me, &[]).map_err(platform)?;
    let key = (me.public_keys().values())
        .find(|k| signer.can_sign_with(k))
        .cloned()
        .ok_or(PlatformError::Identity(IdentityError::KeysMissing {
            purpose: KeyPurpose::Transfer,
        }))?;
    let sent = leased(&lease, async {
        let signed = transition(&me, (address, credits, nonce), &key, &signer, version).await?;
        let st_hash = signed.transaction_id().map_err(internal)?;
        nonces.signed(id, nonce, Signed::Transfer { st_hash });
        Ok::<_, PlatformError>((st_hash, net.broadcast(signed).await))
    })
    .await;
    drop(signer);
    permit.finish(matches!(sent, Ok((_, Ok(_)))));
    let (st_hash, answer) = sent.map_err(CreditsError::from)?;
    let (this, transition) = (Signed::Transfer { st_hash }, hex::encode(st_hash));
    let rivals = (nonces.unconfirmed(id))
        .filter(|(k, _)| *k == nonce)
        .map_or_else(|| vec![this], |(_, all)| all);
    match decide(net, &rivals, (id, nonce), height).await {
        Ok(Some(s)) if s == this => {
            nonces.executed(id, nonce);
            Ok(WithdrawToPlatformOutcome {
                credits,
                to: bech32m(&address, network),
                remaining_credits: answer.ok(),
                transition,
            })
        }
        Ok(Some(s)) => {
            nonces.executed(id, nonce);
            Err(landed_error(s))
        }
        read => {
            if let Err(e) = read {
                tracing::info!("reading the transfer's nonce back: {e}");
            }
            Err(match answer {
                Err(Unsent::Refused(e)) => e,
                _ => PlatformError::BroadcastUnknown {
                    artifact: transition,
                },
            }
            .into())
        }
    }
}

#[cfg(test)]
#[path = "credit_transfer_tests.rs"]
mod tests;
