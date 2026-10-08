//! Invitation links: stash, status, list and forget (DP5-01, DP5-02). Per
//! network, on `NetworkSession`: a link may arrive before any wallet, or
//! any vault, exists (F18).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::dashpay::{BearerSecret, stub};
use super::errors::InvitationError;
use super::payments::Counterparty;
use crate::NetworkSession;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InvitationStatus {
    Valid {
        inviter: Option<Counterparty>,
        funding_duffs: u64,
        contested_allowed: bool,
        expires_at: Option<u64>,
    },
    Claimed,
    Invalid {
        reason: InvitationInvalidReason,
    },
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvitationInvalidReason {
    Malformed,
    WrongNetwork,
    AssetLockNotFound,
}

#[expect(unused_variables, reason = "stubs until DP5-01 and DP5-02")]
impl NetworkSession {
    /// Stores the link as the vault record `invitation/<id>`, or before a
    /// vault exists as a 0600 file in the vault directory that moves into
    /// the vault when it is created (§2.9). Returns the id.
    pub async fn stash_invitation(
        self: &Arc<Self>,
        link: BearerSecret,
    ) -> Result<String, InvitationError> {
        stub("NetworkSession.stash_invitation")
    }

    pub async fn invitation_status(
        self: &Arc<Self>,
        link_id: String,
    ) -> Result<InvitationStatus, InvitationError> {
        stub("NetworkSession.invitation_status")
    }

    /// The ids of the stashed links, oldest first, for the replay after a
    /// restart or onboarding.
    pub async fn pending_invitations(self: &Arc<Self>) -> Result<Vec<String>, InvitationError> {
        stub("NetworkSession.pending_invitations")
    }

    /// Deletes a stashed link: dismissed, or claimed.
    pub async fn forget_invitation(
        self: &Arc<Self>,
        link_id: String,
    ) -> Result<(), InvitationError> {
        stub("NetworkSession.forget_invitation")
    }
}
