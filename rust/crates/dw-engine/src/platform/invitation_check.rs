//! What a stashed invitation link is worth (DP5-02 §1, §2): the checks
//! platform-wallet's `invitation_claim_status` (#4997) runs, on the session
//! rather than a wallet, so a link can be checked before any wallet exists
//! (F18), and with the outpoint the claim spends.
//!
//! At the pin (`rs-platform-wallet/src/wallet/identity/network/invitation.rs`)
//! that call is `inspect_invitation_funding` → `fetch_funding_tx_with_retry` →
//! `inspect_funding_tx`, all private; this module is the same sequence over
//! the library's public pieces (`parse_invitation_uri`,
//! `wif_network_matches`, `voucher_credit_output`) and the SDK's
//! `get_transaction`.

use std::time::Duration;

use dashcore::consensus::Decodable;
use dashcore::ephemerealdata::instant_lock::InstantLock;
use dashcore::{Network, OutPoint, Transaction};
use dpp::prelude::Identifier;
use dw_uri::deeplink::normalize_invitation;
use platform_wallet::wallet::identity::crypto::invitation::{
    ParsedInvitation, parse_invitation_uri, voucher_credit_output, wif_network_matches,
};
use zeroize::Zeroizing;

use super::errors::PlatformError;
use super::invitations::InvitationInvalidReason;
use super::payments::Counterparty;
use super::registration_flow::{CONTESTED_LOCK_DUFFS, identity_for};
use super::trust::BoxFuture;

/// Attempts of the funding transaction's fetch, both byte orders each, as
/// the library's claim (`CLAIM_FETCH_MAX_ATTEMPTS`): a fresh invitation may
/// not have reached the queried DAPI node yet.
pub(crate) const FETCH_ATTEMPTS: u32 = 5;
/// The pause between two attempts (`CLAIM_FETCH_RETRY_DELAY`).
pub(crate) const FETCH_PAUSE: Duration = Duration::from_secs(3);

/// The funding transaction as DAPI reports it. A link waiting for its
/// ChainLock is still `Valid` (the claim waits), as #4997 reports it.
pub(crate) struct Fetched {
    pub(crate) transaction: Transaction,
    /// Whether its block is chain-locked.
    pub(crate) chain_locked: bool,
}

/// The voucher a link spends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Voucher {
    /// The output the claim's identity id derives from.
    pub(crate) outpoint: OutPoint,
    /// Its value: what the claim spends.
    pub(crate) duffs: u64,
    /// Whether the claim can build a ChainLock proof now.
    pub(crate) chain_locked: bool,
}

/// What the checks ask of the network: the seam tests script.
pub(crate) trait InvitationNet: Send + Sync {
    /// The session's network.
    fn network(&self) -> Network;
    /// `Sdk::get_transaction`: `None` when the node does not know `txid`.
    fn transaction(&self, txid: String) -> BoxFuture<Result<Option<Fetched>, PlatformError>>;
    /// Whether Platform has an identity with this id.
    fn identity_exists(&self, id: Identifier) -> BoxFuture<Result<bool, PlatformError>>;
    /// The identity DPNS holds `username` for, with its profile (§2); `None`
    /// when DPNS has no such name.
    fn inviter(&self, username: String) -> BoxFuture<Result<Option<Counterparty>, PlatformError>>;
    /// The pause between two fetch attempts.
    fn pause(&self) -> BoxFuture<()>;
}

/// A link's verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Checked {
    Valid {
        inviter: Option<Counterparty>,
        /// The voucher's credit output: what the claim spends.
        funding_duffs: u64,
        /// The output the claim's identity id derives from.
        outpoint: OutPoint,
    },
    /// An identity exists at the claim's id (one-way, §1).
    Claimed,
    Invalid(InvitationInvalidReason),
}

impl Checked {
    /// A contested name needs at least DP1-02's contested lock (D5).
    pub(crate) fn contested_allowed(funding_duffs: u64) -> bool {
        funding_duffs >= CONTESTED_LOCK_DUFFS
    }
}

/// The link's text, normalized (DP5-01's parser) and parsed by the library.
/// `None` when either refuses it. The normalized copy is zeroed; the parsed
/// link wipes its key on drop.
pub(crate) fn parse(text: &str) -> Option<ParsedInvitation> {
    let normalized = Zeroizing::new(normalize_invitation(text)?);
    parse_invitation_uri(&normalized).ok()
}

/// Checks the stashed link `text`. A transport error fails the call; a
/// verdict is the link's.
pub(crate) async fn check(net: &dyn InvitationNet, text: &str) -> Result<Checked, PlatformError> {
    let Some(link) = parse(text) else {
        return Ok(Checked::Invalid(InvitationInvalidReason::Malformed));
    };
    let Voucher {
        outpoint, duffs, ..
    } = match funding(net, &link).await? {
        Ok(voucher) => voucher,
        Err(reason) => return Ok(Checked::Invalid(reason)),
    };
    if net.identity_exists(identity_for(outpoint)).await? {
        return Ok(Checked::Claimed);
    }
    let username = (link.inviter.as_ref()).and_then(|i| i.username.clone());
    let inviter = match username {
        Some(username) => net.inviter(username).await?,
        None => None,
    };
    Ok(Checked::Valid {
        inviter,
        funding_duffs: duffs,
        outpoint,
    })
}

/// The voucher, from the funding transaction DAPI has for `link`; or why
/// the link cannot be claimed. A transport error fails the call.
pub(crate) async fn funding(
    net: &dyn InvitationNet,
    link: &ParsedInvitation,
) -> Result<Result<Voucher, InvitationInvalidReason>, PlatformError> {
    use InvitationInvalidReason as R;
    if !wif_network_matches(link.voucher_key_network, net.network()) {
        return Ok(Err(R::WrongNetwork));
    }
    let Some(reversed) = reverse_txid(&link.funding_txid) else {
        return Ok(Err(R::Malformed));
    };
    let Some(fetched) = fetch(net, &link.funding_txid, &reversed).await? else {
        return Ok(Err(R::AssetLockNotFound));
    };
    let Some((outpoint, duffs)) = inspect(&fetched.transaction, link, &reversed) else {
        return Ok(Err(R::Malformed));
    };
    Ok(Ok(Voucher {
        outpoint,
        duffs,
        chain_locked: fetched.chain_locked,
    }))
}

/// The link's txid byte-reversed (old iOS links are little-endian); `None`
/// when it is not 32 bytes of hex.
fn reverse_txid(txid: &str) -> Option<String> {
    let mut bytes = <[u8; 32]>::try_from(hex::decode(txid).ok()?).ok()?;
    bytes.reverse();
    Some(hex::encode(bytes))
}

/// `fetch_funding_tx_with_retry`: the canonical id, then the reversed one,
/// [`FETCH_ATTEMPTS`] times with a pause between; a transport error is
/// returned at once (it is not a miss).
async fn fetch(
    net: &dyn InvitationNet,
    txid: &str,
    reversed: &str,
) -> Result<Option<Fetched>, PlatformError> {
    for attempt in 0..FETCH_ATTEMPTS {
        for id in [txid, reversed] {
            if let Some(fetched) = net.transaction(id.to_owned()).await? {
                return Ok(Some(fetched));
            }
        }
        if attempt + 1 < FETCH_ATTEMPTS {
            net.pause().await;
        }
    }
    Ok(None)
}

/// `inspect_funding_tx`: the transaction is the link's (either byte order),
/// an asset lock with a credit output the voucher key controls, and the
/// link's islock, if any, decodes and locks it. The outpoint and its value.
fn inspect(
    transaction: &Transaction,
    link: &ParsedInvitation,
    reversed: &str,
) -> Option<(OutPoint, u64)> {
    let txid = transaction.txid();
    let shown = txid.to_string();
    if shown != link.funding_txid && shown != reversed {
        return None;
    }
    let (vout, output) = voucher_credit_output(transaction, &link.voucher_key).ok()?;
    if let Some(islock) = &link.islock_hex {
        let bytes = hex::decode(islock).ok()?;
        let lock = InstantLock::consensus_decode(&mut bytes.as_slice()).ok()?;
        if lock.txid != txid {
            return None;
        }
    }
    Some((OutPoint::new(txid, vout), output.value))
}

#[cfg(test)]
#[path = "invitation_check_tests.rs"]
mod tests;
