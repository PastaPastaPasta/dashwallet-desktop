//! The masternodes facade (MG-03, `docs/contracts/mn-engine.md`): plain
//! Rust that `dwcli` drives directly and E0-13 binds for the UI. One
//! [`Masternodes`] handle per call site, on the session's shared state
//! (`follow.rs`); the wallet is an argument of each call, since tracked
//! masternodes belong to the network (MG-03 D13).
//!
//! The ProTx calls (`protx.rs`, MG-04) sign the owner's side of a ProRegTx
//! under a `MasternodeOp` grant; no call signs as operator, withdraws
//! credits (MG-13), or takes, holds or returns an operator secret (MG-01's
//! guard).

mod aggregate;
mod errors;
pub(crate) mod follow;
mod protx;
mod protx_records;
mod reads;
mod records;
mod shared;
mod shared_records;
mod tracking;

use std::sync::Arc;

pub use errors::MasternodesError;
pub use protx_records::{
    CollateralChoice, CollateralKind, EvonodeTerms, KeepOr, KeyPick, OperatorChoice,
    OwnerKeySource, PayoutChoice, PlatformFacts, PlatformNodeChoice, ProTxKeys, ProTxOperation,
    ProTxOperationKind, ProTxOperationState, ProTxTerms, RegistrationPreview, RegistrationRequest,
    SignRequest, UpdateRegistrarPreview, UpdateRegistrarRequest, VotingChoice,
};
pub use records::{
    AttachedKey, AttachedKeyRole, CollateralCoin, CollateralState, EvonodeCredits, HeldShares,
    KeyCheck, ListFacts, ListedMasternode, MasternodeDetail, MasternodeListState,
    MasternodeOverview, MasternodePayout, MasternodeRoles, MasternodeRow, MasternodeStatus,
    MasternodeType, RegistrationFacts, UntrackOutcome,
};
pub use shared_records::{
    Activation, DeadReason, JoinSharedRequest, KeyBase, RewardChoice, ShareRow, ShareSlot,
    ShareTerms, ShareTermsView, SharedCapability, SharedImport, SharedKind, SharedMessage,
    SharedNext, SharedOutcome, SharedRole, SharedSession, SharedStage, SharedWarning,
    StartSharedRequest, UpdateSharePreview, UpdateShareRequest,
};
pub use tracking::MAX_MASTERNODE_LABEL;

use crate::{EngineError, NetworkSession};

/// Masternodes of one network session. A cheap handle: the state lives in
/// the session, so any number of handles may exist (mn-engine §0).
#[derive(Clone)]
pub struct Masternodes {
    pub(super) session: Arc<NetworkSession>,
    /// The shared-masternode sending override (MG-04b r2-m4, DEC-678):
    /// only the test-support constructor sets it.
    #[cfg(any(test, all(debug_assertions, feature = "test-support")))]
    shared_send: bool,
}

impl NetworkSession {
    /// The masternodes facade of this session.
    pub fn masternodes(self: &Arc<Self>) -> Masternodes {
        Masternodes {
            session: Arc::clone(self),
            #[cfg(any(test, all(debug_assertions, feature = "test-support")))]
            shared_send: shared::SHARED_SENDING,
        }
    }
}

impl Masternodes {
    /// Runs `f` on the engine runtime under one operation guard.
    async fn op<T, F, Fut>(&self, f: F) -> Result<T, MasternodesError>
    where
        T: Send + 'static,
        F: FnOnce(Arc<NetworkSession>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, MasternodesError>> + Send,
    {
        let session = Arc::clone(&self.session);
        self.session
            .on_runtime(async move {
                let _op = session.enter().await?;
                Ok(f(Arc::clone(&session)).await)
            })
            .await?
    }
}

/// Wire-order bytes of a display-order ProTx hash (64 hex characters). The
/// refusal doesn't repeat the input, which may be a pasted key.
fn parse_pro_tx_hash(hex_hash: &str) -> Result<[u8; 32], EngineError> {
    let mut bytes = [0u8; 32];
    hex::decode_to_slice(hex_hash, &mut bytes)
        .map_err(|_| EngineError::InvalidArgument("not a ProTx hash (64 hex)".into()))?;
    bytes.reverse();
    Ok(bytes)
}

/// Display-order lower-case hex of wire-order hash bytes.
fn display_hex(wire: &[u8; 32]) -> String {
    let mut display = *wire;
    display.reverse();
    hex::encode(display)
}

/// Core's `COutPoint::ToStringShort`.
fn outpoint_string(o: &dashcore::OutPoint) -> String {
    format!("{}-{}", o.txid, o.vout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pro_tx_hashes_are_display_hex_of_reversed_wire_bytes() {
        let mut wire = [0u8; 32];
        wire[0] = 0xab;
        let shown = display_hex(&wire);
        assert!(shown.ends_with("ab"));
        assert_eq!(parse_pro_tx_hash(&shown).unwrap(), wire);
        assert_eq!(parse_pro_tx_hash(&shown.to_uppercase()).unwrap(), wire);
        for bad in ["", "ab", &"g".repeat(64), &"a".repeat(66)] {
            assert_eq!(
                parse_pro_tx_hash(bad).unwrap_err().code(),
                "invalid_argument"
            );
        }
    }
}
