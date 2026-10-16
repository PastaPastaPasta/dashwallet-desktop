//! The masternodes facade (MG-03, `docs/contracts/mn-engine.md`): plain
//! Rust that `dwcli` drives directly and E0-13 binds for the UI. One
//! [`Masternodes`] handle per call site, on the session's shared state
//! (`follow.rs`); the wallet is an argument of each call, since tracked
//! masternodes belong to the network (MG-03 D13).
//!
//! Read and track only: no call signs a ProTx (MG-04), withdraws credits
//! (MG-13), or takes, holds or returns an operator key (MG-01's guard).

mod aggregate;
mod errors;
pub(crate) mod follow;
mod reads;
mod records;
mod tracking;

use std::sync::Arc;

pub use errors::MasternodesError;
pub use records::{
    AttachedKeyRole, CollateralCoin, CollateralState, EvonodeCredits, KeyCheck, ListFacts,
    ListedMasternode, MasternodeDetail, MasternodeListState, MasternodeOverview, MasternodePayout,
    MasternodeRoles, MasternodeRow, MasternodeStatus, MasternodeType, RegistrationFacts,
    UntrackOutcome,
};
pub use tracking::MAX_MASTERNODE_LABEL;

use crate::{EngineError, NetworkSession};

/// Masternodes of one network session. A cheap handle: the state lives in
/// the session, so any number of handles may exist (mn-engine §0).
#[derive(Clone)]
pub struct Masternodes {
    pub(super) session: Arc<NetworkSession>,
}

impl NetworkSession {
    /// The masternodes facade of this session.
    pub fn masternodes(self: &Arc<Self>) -> Masternodes {
        Masternodes {
            session: Arc::clone(self),
        }
    }
}

/// Wire-order bytes of a display-order ProTx hash (64 hex characters).
fn parse_pro_tx_hash(hex_hash: &str) -> Result<[u8; 32], EngineError> {
    let invalid = || EngineError::InvalidArgument(format!("not a ProTx hash: {hex_hash:?}"));
    if hex_hash.len() != 64 {
        return Err(invalid());
    }
    let mut bytes: [u8; 32] = hex::decode(hex_hash)
        .map_err(|_| invalid())?
        .try_into()
        .map_err(|_| invalid())?;
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

/// A contract stub's body (`masternodes.not_implemented{call}`).
#[allow(dead_code, reason = "no call is a stub once MG-03 is complete")]
fn stub<T>(call: &str) -> Result<T, MasternodesError> {
    Err(MasternodesError::NotImplemented { call: call.into() })
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
