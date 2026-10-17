//! Link intake and invitation links: stash, list, take, status and forget
//! (DP5-01, DP5-02). Per network, on `NetworkSession`: a link may arrive
//! before any wallet, or any vault, exists (F18).
//!
//! The stash itself is the vault's (`dw_vault`, `vault/links.rs`; design
//! `docs/design/DP5-01-link-intake.md`): one sealed file per link, a
//! plaintext 0600 file only before a vault exists, bounded, with a time to
//! live, deduplicated, and taken once. Here the engine classifies each link
//! itself (it never trusts a host's word for what a link is), and keeps
//! invitations inside: an invitation link is a bearer secret, so no call
//! hands one back (DP5-02 claims by id).

use std::sync::Arc;

use dw_uri::deeplink::{DeepLink, classify_link, normalize_invitation};
use dw_vault::{ALL_LINK_KINDS, StashedLinkKind as VaultKind, StashedLinkMeta};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::dashpay::{BearerSecret, stub};
use super::errors::{InvitationError, PlatformError};
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

/// What a stashed link is, as the engine classified it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StashedLinkKind {
    /// A payment link: `dash:`, `pay:`, `dashwallet://pay=…`, or a BIP73
    /// `http(s)` payment string, with an address.
    Payment,
    /// A DashPay user link (`dashpay://user`).
    Contact,
    /// A DashPay invitation (`dashpay://invite`, the invitations site, or an
    /// AppsFlyer link wrapping either). Never handed back.
    Invitation,
}

/// A stashed link without its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StashedLink {
    /// The engine-issued id (32 lower-case hex characters).
    pub id: String,
    pub kind: StashedLinkKind,
    /// UNIX seconds.
    pub stashed_at: u64,
    /// UNIX seconds; the link is deleted from then on.
    pub expires_at: u64,
}

/// What [`NetworkSession::take_stashed_link`] found.
#[derive(Debug)]
pub enum TakenLink {
    /// A payment or contact link's text, now deleted from the stash.
    Link(BearerSecret),
    /// An invitation: never handed back nor opened, and it stays (DP5-02
    /// claims it by id).
    Invitation,
}

impl From<VaultKind> for StashedLinkKind {
    fn from(k: VaultKind) -> Self {
        match k {
            VaultKind::Payment => Self::Payment,
            VaultKind::Contact => Self::Contact,
            VaultKind::Invitation => Self::Invitation,
        }
    }
}

impl From<StashedLinkKind> for VaultKind {
    fn from(k: StashedLinkKind) -> Self {
        match k {
            StashedLinkKind::Payment => Self::Payment,
            StashedLinkKind::Contact => Self::Contact,
            StashedLinkKind::Invitation => Self::Invitation,
        }
    }
}

impl From<StashedLinkMeta> for StashedLink {
    fn from(m: StashedLinkMeta) -> Self {
        Self {
            id: m.id,
            kind: m.kind.into(),
            stashed_at: m.stashed_at,
            expires_at: m.expires_at,
        }
    }
}

/// Whether log redaction could miss part of `link` (D14): a kept link is
/// one line (redaction ends a link there) that the redaction cuts from its
/// first character on.
fn unredactable(link: &str) -> bool {
    let link = link.trim();
    link.chars().any(char::is_control) || !dw_uri::redact::redacted_whole(link)
}

/// The kind of `link` the stash keeps, or `None`. Every copy the parser
/// makes is dropped here; an invitation's normalized copy is zeroed.
pub(crate) fn stash_kind(link: &str) -> Option<StashedLinkKind> {
    if unredactable(link) {
        return None;
    }
    match classify_link(link) {
        DeepLink::Payment(req) => req.address.is_some().then_some(StashedLinkKind::Payment),
        DeepLink::DashPayUser(_) => Some(StashedLinkKind::Contact),
        DeepLink::Invitation(normalized) => {
            drop(Zeroizing::new(normalized));
            Some(StashedLinkKind::Invitation)
        }
        _ => None,
    }
}

/// Whether `link` is an invitation link, checked without the payment
/// parser: text handed in as an invitation is a bearer secret, and the
/// payment parser keeps unzeroed copies of what it reads (D24).
fn is_invitation(link: &str) -> bool {
    !unredactable(link) && normalize_invitation(link).map(Zeroizing::new).is_some()
}

impl NetworkSession {
    /// Runs a stash call on the vault, on the engine's blocking pool.
    async fn stash_call<T, F>(self: &Arc<Self>, f: F) -> Result<T, PlatformError>
    where
        F: FnOnce(&dw_vault::Vault) -> Result<T, dw_vault::VaultError> + Send + 'static,
        T: Send + 'static,
    {
        Ok(self.vault_op(f).await?)
    }

    /// Stashes `link` as `kind`, which the caller classified.
    async fn stash_as(
        self: &Arc<Self>,
        kind: StashedLinkKind,
        link: BearerSecret,
    ) -> Result<StashedLink, PlatformError> {
        let meta = self
            .stash_call(move |v| v.stash_link(kind.into(), link.expose()))
            .await?;
        Ok(meta.into())
    }

    /// Keeps a payment, contact or invitation link for later (DP5-01): the
    /// stash entry it now has, or already had (the same link again). Any
    /// other text is `invalid_argument`, and no error quotes the link.
    pub async fn stash_link(
        self: &Arc<Self>,
        link: BearerSecret,
    ) -> Result<StashedLink, PlatformError> {
        let kind = stash_kind(link.expose()).ok_or_else(|| PlatformError::InvalidArgument {
            detail: "not a payment, contact or invitation link".into(),
        })?;
        self.stash_as(kind, link).await
    }

    /// The stashed links, oldest first, without their text. Works locked.
    pub async fn stashed_links(self: &Arc<Self>) -> Result<Vec<StashedLink>, PlatformError> {
        let links = self.stash_call(|v| v.stashed_links()).await?;
        Ok(links.into_iter().map(Into::into).collect())
    }

    /// Takes a stashed payment or contact link: its text, once; `None` when
    /// there is no such link any more. An invitation is
    /// [`TakenLink::Invitation`]: never handed back nor opened, and it
    /// stays, in every lock state. Otherwise a locked vault is
    /// `platform.signer_unavailable` and keeps the link.
    pub async fn take_stashed_link(
        self: &Arc<Self>,
        link_id: String,
    ) -> Result<Option<TakenLink>, PlatformError> {
        const HANDED_BACK: &[VaultKind] = &[VaultKind::Payment, VaultKind::Contact];
        self.stash_call(move |v| match v.take_stashed_link(&link_id, HANDED_BACK) {
            Ok(taken) => {
                Ok(taken.map(|(_, text)| TakenLink::Link(BearerSecret::new(String::clone(&text)))))
            }
            // The one kind not handed back.
            Err(dw_vault::VaultError::InvalidArgument(d)) if d == dw_vault::OTHER_KIND => {
                Ok(Some(TakenLink::Invitation))
            }
            Err(e) => Err(e),
        })
        .await
    }

    /// Deletes a stashed link of any kind without reading it: dismissed.
    /// Works locked; an id the stash no longer has is not an error.
    pub async fn forget_stashed_link(
        self: &Arc<Self>,
        link_id: String,
    ) -> Result<(), PlatformError> {
        self.stash_call(move |v| v.forget_stashed_link(&link_id, ALL_LINK_KINDS))
            .await?;
        Ok(())
    }

    /// Stores the link in the vault's link stash, sealed to the vault's key,
    /// or before a vault exists as a 0600 file in the vault directory that
    /// the vault seals when it is created (§2.9). Returns the id. Anything
    /// but an invitation link is `invitation.invalid`.
    pub async fn stash_invitation(
        self: &Arc<Self>,
        link: BearerSecret,
    ) -> Result<String, InvitationError> {
        // The invitation parser alone proves the kind: the full classifier
        // copies what it reads unzeroed (D24).
        if !is_invitation(link.expose()) {
            return Err(InvitationError::Invalid);
        }
        Ok(self.stash_as(StashedLinkKind::Invitation, link).await?.id)
    }

    #[expect(unused_variables, reason = "stub until DP5-02")]
    pub async fn invitation_status(
        self: &Arc<Self>,
        link_id: String,
    ) -> Result<InvitationStatus, InvitationError> {
        stub("NetworkSession.invitation_status")
    }

    /// The ids of the stashed links, oldest first, for the replay after a
    /// restart or onboarding.
    pub async fn pending_invitations(self: &Arc<Self>) -> Result<Vec<String>, InvitationError> {
        Ok(self
            .stashed_links()
            .await?
            .into_iter()
            .filter(|l| l.kind == StashedLinkKind::Invitation)
            .map(|l| l.id)
            .collect())
    }

    /// Deletes a stashed invitation: dismissed, or claimed. An id the stash
    /// no longer has is not an error; one of another kind of link is
    /// `invitation.invalid`, and stays.
    pub async fn forget_invitation(
        self: &Arc<Self>,
        link_id: String,
    ) -> Result<(), InvitationError> {
        let found = self
            .stash_call(move |v| v.forget_stashed_link(&link_id, &[VaultKind::Invitation]))
            .await?;
        match found {
            Some(kind) if kind != VaultKind::Invitation => Err(InvitationError::Invalid),
            _ => Ok(()),
        }
    }
}
