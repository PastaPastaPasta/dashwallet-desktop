//! Contact requests (DP2-02, roadmap row 164): eligibility, send, accept,
//! ignore and unignore, and "Enable DashPay keys", behind the facade calls
//! of `contacts.rs`. Not part of the facade.
//!
//! Every write refuses what it can before it redeems a grant, in this
//! order: the identity and the other user (`eligibility`), the identity's
//! credits against the action's quote, the keys its flow signs with and a
//! scan's expiry, then the grant or lease. It signs under a lease
//! ([`write_lease`]) with exactly the keys its transition needs
//! ([`flow_keys`]), charges the quote to the lease's Credits budget just
//! before it goes out, counted as a running call ([`leased`]), and holds the
//! identity's write lock throughout (`identity_writes.rs`).
//!
//! Fees (DEC-195): Platform meters a transition's fee against its state
//! when it executes, so no quote can equal the charge. The quote is a bound
//! (`grant_request`, E0-04's per-action credits); what was charged is the
//! proved balance before minus after, which the key upgrade logs and T2
//! records against the quote.

use std::future::Future;
use std::sync::Arc;

use dpp::identity::accessors::IdentityGettersV0;
use dpp::identity::identity_public_key::accessors::v0::IdentityPublicKeyGettersV0;
use dpp::identity::signer::Signer;
use dpp::identity::{Identity, IdentityPublicKey, KeyID, KeyType, Purpose, SecurityLevel};
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use platform_wallet::{AutoAcceptProofSource, ContactRequest, PlatformWallet};

use super::contacts::{Eligibility, RequestOutcome};
use super::contacts_model::identifier;
use super::errors::{ContactError, IdentityError, PlatformError};
use super::flows::{BudgetPurpose, CONTACT_REQUEST_CREDITS, FlowKind, KEY_UPDATE_CREDITS};
use super::identity::KeyPurpose;
use super::identity_writes::identity_locks;
use super::journal::request_ref;
use super::keys_policy::{
    derive_keys, is_dashpay_recipient, missing_dashpay_purposes, upgrade_specs,
};
use super::lease::Lease;
use super::names::{NameAuthority, check_credit_cap};
use super::signers::{VaultContactCrypto, VaultIdentitySigner};
use super::trust::TrustProvider;
use super::users::send_seam::ScanClaim;
use crate::{NetworkSession, WalletId};

#[path = "requests_net.rs"]
mod net;
pub(super) use net::{RequestNet, SdkRequests};

/// One call's sides: one of the wallet's identities and another user.
#[derive(Debug, Clone, Copy)]
pub(super) struct Pair {
    pub(super) wallet: WalletId,
    pub(super) identity: Identifier,
    pub(super) contact: Identifier,
}

impl Pair {
    pub(super) fn parse(
        wallet: WalletId,
        identity: &str,
        contact: &str,
    ) -> Result<Self, ContactError> {
        Ok(Self {
            wallet,
            identity: identifier(identity)?,
            contact: identifier(contact)?,
        })
    }
}

fn b58(id: &Identifier) -> String {
    id.to_string(Encoding::Base58)
}

fn not_found() -> PlatformError {
    IdentityError::NotFound.into()
}

/// The wallet's identity as platform-wallet holds it, and what its state
/// says of the other user.
struct Own {
    /// Its DIP-13 index; `None` for an identity the wallet only watches.
    index: Option<u32>,
    identity: Identity,
    established: bool,
    sent: bool,
    incoming: Option<ContactRequest>,
}

impl Own {
    /// `identity` of the wallet, else `identity.not_found`.
    async fn of(wallet: &PlatformWallet, pair: &Pair) -> Result<Self, ContactError> {
        let state = wallet.state().await;
        let m = (state.identity_manager.managed_identity(&pair.identity))
            .filter(|m| m.wallet_id == Some(wallet.wallet_id()))
            .ok_or_else(not_found)?;
        let dp = m.dashpay();
        Ok(Self {
            index: m.identity_index,
            identity: m.identity.clone(),
            established: dp.established_contacts().contains_key(&pair.contact),
            sent: dp.sent_contact_requests().contains_key(&pair.contact),
            incoming: dp.incoming_contact_requests().get(&pair.contact).cloned(),
        })
    }

    /// It has an enabled ECDSA key for both DashPay purposes.
    fn has_dashpay_keys(&self) -> bool {
        missing_dashpay_purposes(self.identity.public_keys().values()).is_empty()
    }

    /// What the wallet's own state answers (m4 §2.4 `eligibility`); `Ok`:
    /// nothing here refuses, Platform has the rest.
    fn local(&self, contact: &Identifier) -> Eligibility {
        if *contact == self.identity.id() {
            Eligibility::IsSelf
        } else if self.established {
            Eligibility::AlreadyContact
        } else if self.sent {
            Eligibility::PendingOutgoing
        } else if self.incoming.is_some() {
            Eligibility::PendingIncoming
        } else if !self.has_dashpay_keys() {
            Eligibility::NoDashPayKeys
        } else {
            Eligibility::Ok
        }
    }

    /// The whole answer: [`Own::local`], then the contact's key set from
    /// Platform.
    async fn eligibility(
        &self,
        net: &impl RequestNet,
        contact: Identifier,
    ) -> Result<Eligibility, ContactError> {
        match self.local(&contact) {
            Eligibility::Ok => recipient(net, contact).await,
            refusal => Ok(refusal),
        }
    }

    fn index(&self) -> Result<u32, PlatformError> {
        self.index.ok_or(PlatformError::SignerUnavailable)
    }
}

/// Whether Platform's copy of `contact` can receive a request
/// (`keys_policy::is_dashpay_recipient`).
async fn recipient(
    net: &impl RequestNet,
    contact: Identifier,
) -> Result<Eligibility, ContactError> {
    let found = net.identity(contact).await?.ok_or_else(not_found)?;
    Ok(if is_dashpay_recipient(found.public_keys().values()) {
        Eligibility::Ok
    } else {
        Eligibility::ContactCannotReceive
    })
}

/// `DashPay::eligibility`: the wallet's state first, then the contact's
/// key set from Platform. Reads only.
pub(super) async fn eligibility(
    net: &impl RequestNet,
    wallet: &PlatformWallet,
    pair: Pair,
) -> Result<Eligibility, ContactError> {
    Own::of(wallet, &pair)
        .await?
        .eligibility(net, pair.contact)
        .await
}

/// Refuses before any grant is redeemed when the identity's credits are
/// below `quote` (`platform.insufficient_credits`).
fn cover(available: u64, quote: u64) -> Result<(), PlatformError> {
    if available < quote {
        return Err(PlatformError::InsufficientCredits {
            needed: quote,
            available,
        });
    }
    Ok(())
}

/// The lease a write signs under, once nothing else refuses, consuming
/// nothing before its checks pass:
/// - a lease id (`begin_flow`) of the wallet for one of `flows` (another
///   flow's lease is `platform.grant_invalid`: its keys are not this
///   write's), whose Credits budget left covers `quote`;
/// - a vault `PlatformOp` grant whose `max_credits` covers `quote`
///   (names.rs `check_credit_cap`), redeemed into a lease of `flows[0]`
///   that ends when the call drops it.
///
/// A budget below `quote` is `platform.grant_exceeded`, none at all
/// `platform.needs_grant{Credits}`.
async fn write_lease(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    grant: &str,
    flows: &[FlowKind],
    quote: u64,
) -> Result<Lease, ContactError> {
    let Some(lease) = session.lease_for(&wallet, grant) else {
        check_credit_cap(
            session,
            wallet,
            &NameAuthority::Grant(grant.to_owned()),
            quote,
        )
        .await?;
        let grants = [grant.to_owned()];
        let lease = session.begin_lease(wallet, flows[0], &grants).await;
        return Ok(lease.map_err(PlatformError::from)?);
    };
    let lease = lease.map_err(PlatformError::from)?;
    if !flows.contains(&lease.flow()) {
        return Err(PlatformError::GrantInvalid.into());
    }
    let view = lease.view().ok_or(PlatformError::GrantInvalid)?;
    let credits = (view.budgets.iter()).find(|b| b.purpose == BudgetPurpose::Credits);
    let Some(credits) = credits.filter(|b| b.ceiling > 0) else {
        return Err(PlatformError::NeedsGrant {
            purpose: BudgetPurpose::Credits,
        }
        .into());
    };
    let remaining = credits.ceiling.saturating_sub(credits.spent);
    if quote > remaining {
        return Err(PlatformError::GrantExceeded {
            purpose: BudgetPurpose::Credits,
            needed: quote,
            remaining,
        }
        .into());
    }
    Ok(lease)
}

/// Runs `call`, a library write, under `lease`, in its dispatch scope.
pub(super) async fn leased<F: Future>(lease: &Lease, call: F) -> F::Output {
    lease.scope(call).await
}

/// The first enabled ECDSA AUTHENTICATION key at one of `levels` that is
/// `usable`, in key-id order, as platform-wallet picks its signing key (the
/// first its signer's `can_sign_with` accepts).
fn first_auth_key(
    identity: &Identity,
    levels: &[SecurityLevel],
    usable: impl Fn(&IdentityPublicKey) -> bool,
) -> Option<KeyID> {
    (identity.public_keys().values())
        .find(|k| {
            k.purpose() == Purpose::AUTHENTICATION
                && levels.contains(&k.security_level())
                && k.key_type() == KeyType::ECDSA_SECP256K1
                && !k.is_disabled()
                && usable(k)
        })
        .map(|k| k.id())
}

/// The keys `flow`'s transition signs with, and no others:
/// - a contact request or accept: the document key, the identity's first
///   HIGH or CRITICAL authentication key that is `usable` (the vault holds
///   it; DPP refuses the master on document writes; the DashPay keys only do
///   ECDH, in the crypto signer);
/// - a key upgrade: the master key, which signs the IdentityUpdate, and the
///   `added` keys, each of which signs its own proof of possession (dpp
///   `IdentityUpdateTransition::try_from_identity_with_signer`).
///
/// Any other flow writes nothing here: `platform.grant_invalid`. No such
/// key: `identity.keys_missing`, which a write checks with every key
/// `usable` before it redeems anything.
fn flow_keys(
    flow: FlowKind,
    identity: &Identity,
    added: &[KeyID],
    usable: impl Fn(&IdentityPublicKey) -> bool,
) -> Result<Vec<KeyID>, ContactError> {
    let missing = || {
        PlatformError::from(IdentityError::KeysMissing {
            purpose: KeyPurpose::Authentication,
        })
    };
    match flow {
        FlowKind::ContactRequest | FlowKind::Accept | FlowKind::AcceptAndPay => {
            let levels = [SecurityLevel::HIGH, SecurityLevel::CRITICAL];
            let key = first_auth_key(identity, &levels, usable);
            Ok(vec![key.ok_or_else(missing)?])
        }
        FlowKind::EnableDashPayKeys => {
            let master = first_auth_key(identity, &[SecurityLevel::MASTER], usable);
            let master = master.ok_or_else(missing)?;
            Ok(std::iter::once(master)
                .chain(added.iter().copied())
                .collect())
        }
        _ => Err(PlatformError::GrantInvalid.into()),
    }
}

/// The lease's identity signer for the identity at `index`, narrowed to
/// [`flow_keys`] of the lease's flow, chosen among the keys the vault holds.
fn flow_signer(
    lease: &Lease,
    index: u32,
    identity: &Identity,
    added: &[KeyID],
) -> Result<VaultIdentitySigner, ContactError> {
    let signer = lease
        .identity_signer(&[index])
        .map_err(PlatformError::from)?;
    let keys = flow_keys(lease.flow(), identity, added, |k| signer.can_sign_with(k))?;
    Ok(signer.only_keys(keys))
}

/// Charges `quote` to the lease's Credits budget, right before the write it
/// pays for goes out, and keeps it whatever the write's outcome (fail
/// closed: the quote is the bound, and one lease id pays for one write's
/// quote only once).
fn charge(lease: &Lease, quote: u64) -> Result<(), PlatformError> {
    lease.charge_credits(quote)?;
    Ok(())
}

/// What a contact-request write needs once its own checks pass, refused
/// before anything is redeemed where it can be: the identity's credits
/// against the quote and its document key, then the lease of one of
/// `flows`, its signer (the document key alone) and its contact crypto.
/// `ready` runs last before the grant is redeemed (a scan's expiry).
async fn prepare(
    net: &impl RequestNet,
    session: &Arc<NetworkSession>,
    own: &Own,
    wallet: WalletId,
    grant: &str,
    flows: &[FlowKind],
    ready: impl FnOnce() -> Result<(), ContactError>,
) -> Result<(Lease, VaultIdentitySigner, VaultContactCrypto), ContactError> {
    let index = own.index()?;
    let id = own.identity.id();
    let balance = net.balance(id).await?.ok_or_else(not_found)?;
    cover(balance, CONTACT_REQUEST_CREDITS)?;
    flow_keys(flows[0], &own.identity, &[], |_| true)?;
    ready()?;
    let lease = write_lease(session, wallet, grant, flows, CONTACT_REQUEST_CREDITS).await?;
    let signer = flow_signer(&lease, index, &own.identity, &[])?;
    let crypto = lease.contact_crypto().map_err(PlatformError::from)?;
    Ok((lease, signer, crypto))
}

/// A request write's outcome; after a failure, whether `pair.identity`'s
/// request to `pair.contact` is on Platform after all, as names.rs reads a
/// label again (platform-wallet flattens the SDK's errors into text). Until
/// E0-04's fence wiring in the library (P3), a failure that did not land is
/// the library's error, not `platform.broadcast_unknown`.
async fn read_back(
    net: &impl RequestNet,
    wallet: &PlatformWallet,
    pair: &Pair,
    sent: Result<ContactRequest, PlatformError>,
) -> Result<ContactRequest, ContactError> {
    let e = match sent {
        Ok(request) => return Ok(request),
        Err(e) => e,
    };
    tracing::info!(contact = %b58(&pair.contact), "contact request write failed: {e}");
    match net.sent_to(wallet, pair.identity, pair.contact).await {
        Ok(Some(request)) => {
            remember_sent(wallet, &request).await;
            Ok(request)
        }
        Ok(None) => Err(e.into()),
        Err(again) => {
            tracing::info!("after the failure: {again}");
            Err(e.into())
        }
    }
}

/// Records `request`, which landed though its write failed, as the library
/// records one it sent: eligibility and the contact list see it before the
/// next DashPay pass. A failure to persist is logged; the pass records it.
async fn remember_sent(wallet: &PlatformWallet, request: &ContactRequest) {
    let mut state = wallet.state_mut().await;
    let Some(managed) = (state.identity_manager).managed_identity_mut(&request.sender_id) else {
        return;
    };
    if let Err(e) = managed.add_sent_contact_request(request.clone(), wallet.persister()) {
        tracing::info!("recording the request that landed: {e}");
    }
}

/// How long before its expiry a scan is refused (`contact.scan_expired`)
/// before a grant is redeemed: the key goes out with a request Platform
/// receives a little later, and a scan that expires on the way is spent.
const SCAN_MARGIN_SECS: u64 = 60;

/// A send from a scan whose recipient was shown on the fallback's word
/// alone waits for masternode sync: then the library's fetch of the
/// recipient is proof-checked against SPV's quorums, which confirms it
/// (m4 §2.4, Q8). `platform.context_unavailable` until then; any other send
/// goes.
pub(super) fn wait_to_confirm(
    claim: Option<&ScanClaim>,
    trust: &TrustProvider,
) -> Result<(), PlatformError> {
    match claim {
        Some(claim) if claim.recipient_unverified && trust.writes_wait() => {
            Err(PlatformError::ContextUnavailable)
        }
        _ => Ok(()),
    }
}

/// `DashPay::send_request` (m4 §2.4). The scan is claimed first and given
/// up for its proof only once nothing else refuses (`users::send_seam`).
pub(super) async fn send(
    net: &impl RequestNet,
    session: &Arc<NetworkSession>,
    wallet: &PlatformWallet,
    wallet_id: WalletId,
    (identity, to, scan): (String, String, Option<String>),
    grant: String,
) -> Result<RequestOutcome, ContactError> {
    // A refusal below drops the claim, which releases the scan.
    let intent = session.send_request_intent(wallet_id, &identity, &to, scan.as_deref())?;
    let pair = Pair {
        wallet: wallet_id,
        identity: intent.sender,
        contact: intent.recipient,
    };
    let locks = identity_locks(pair.wallet, pair.identity);
    let _write = locks.write.lock().await;
    let own = Own::of(wallet, &pair).await?;
    match own.eligibility(net, pair.contact).await? {
        Eligibility::Ok => {}
        reason => return Err(ContactError::Ineligible { reason }),
    }
    wait_to_confirm(intent.auto_accept.as_ref(), &session.trust)?;
    let flows = [FlowKind::ContactRequest];
    let expiry = intent.auto_accept.as_ref().map(|c| u64::from(c.expiry));
    let ready = || match expiry {
        Some(expiry) if expiry <= crate::events::unix_now() + SCAN_MARGIN_SECS => {
            Err(ContactError::ScanExpired)
        }
        _ => Ok(()),
    };
    let (lease, signer, crypto) =
        prepare(net, session, &own, wallet_id, &grant, &flows, ready).await?;
    // The last step that may refuse: the key goes out with the request, so
    // from here the scan is spent whatever happens (fail closed).
    let (proof, _spend) = match intent.auto_accept {
        Some(claim) => {
            let (proof, spend) = claim.proof_source(crate::events::unix_now())?;
            (proof, Some(spend))
        }
        None => (AutoAcceptProofSource::None, None),
    };
    charge(&lease, CONTACT_REQUEST_CREDITS)?;
    let sent = leased(
        &lease,
        net.send(wallet, pair.identity, pair.contact, proof, &signer, &crypto),
    )
    .await;
    let request = request_ref(&read_back(net, wallet, &pair, sent).await?);
    // Their request landed in the meantime and the library joined them.
    let established = Own::of(wallet, &pair).await.is_ok_and(|o| o.established);
    Ok(if established {
        RequestOutcome::Established { request }
    } else {
        RequestOutcome::Pending { request }
    })
}

/// `DashPay::accept_request`: sends the reverse request (or adopts one
/// already on Platform, as the library does) for `pair.contact`'s request.
pub(super) async fn accept(
    net: &impl RequestNet,
    session: &Arc<NetworkSession>,
    wallet: &PlatformWallet,
    pair: Pair,
    grant: String,
) -> Result<RequestOutcome, ContactError> {
    if pair.identity == pair.contact {
        return Err(ContactError::IsSelf);
    }
    let locks = identity_locks(pair.wallet, pair.identity);
    let _write = locks.write.lock().await;
    let mut own = Own::of(wallet, &pair).await?;
    if own.established {
        return Err(ContactError::AlreadyContact);
    }
    let request = (own.incoming.take()).ok_or_else(|| PlatformError::InvalidArgument {
        detail: "no contact request from this identity".into(),
    })?;
    if !own.has_dashpay_keys() {
        return Err(ContactError::Ineligible {
            reason: Eligibility::NoDashPayKeys,
        });
    }
    let flows = [FlowKind::Accept, FlowKind::AcceptAndPay];
    let (lease, signer, crypto) =
        prepare(net, session, &own, pair.wallet, &grant, &flows, || Ok(())).await?;
    charge(&lease, CONTACT_REQUEST_CREDITS)?;
    let accepted = leased(&lease, net.accept(wallet, &request, &signer, &crypto)).await;
    let ours = read_back(net, wallet, &pair, accepted).await?;
    Ok(RequestOutcome::Established {
        request: request_ref(&ours),
    })
}

/// `DashPay::ignore` and `unignore`: local only, no grant (the library
/// persists the ignored senders; the changeset tap signals `Contacts`).
/// Under the identity's write lock, so an accept never runs on a request
/// being ignored.
pub(super) async fn set_ignored(
    wallet: &PlatformWallet,
    pair: Pair,
    ignored: bool,
) -> Result<(), ContactError> {
    if pair.identity == pair.contact {
        return Err(ContactError::IsSelf);
    }
    let locks = identity_locks(pair.wallet, pair.identity);
    let _write = locks.write.lock().await;
    Own::of(wallet, &pair).await?;
    let dashpay = wallet.identity().dashpay();
    let done = if ignored {
        dashpay
            .ignore_contact_sender(&pair.identity, &pair.contact)
            .await
    } else {
        dashpay
            .unignore_contact_sender(&pair.identity, &pair.contact)
            .await
    };
    Ok(done.map_err(PlatformError::from)?)
}

/// Logs what the key upgrade cost: the proved balance before minus after.
/// A difference that is negative or above the quote is not this write's
/// fee alone (something else moved the balance) and is logged as such.
fn note_charge(identity: &Identifier, before: u64, after: u64) {
    let (action, quote, identity) = ("enable_dashpay_keys", KEY_UPDATE_CREDITS, b58(identity));
    match before.checked_sub(after) {
        Some(charged) if charged <= quote => {
            tracing::info!(
                action,
                identity,
                quote,
                before,
                after,
                charged,
                "credits charged"
            );
        }
        _ => tracing::warn!(
            action,
            identity,
            quote,
            before,
            after,
            "the balance moved by more than this write"
        ),
    }
}

/// `DashPay::enable_dashpay_keys` (F9): adds the DashPay keys the identity
/// lacks on Platform. Nothing missing: `Ok`, nothing sent, the grant left
/// unused.
pub(super) async fn enable_keys(
    net: &impl RequestNet,
    session: &Arc<NetworkSession>,
    wallet: &PlatformWallet,
    wallet_id: WalletId,
    identity: Identifier,
    grant: String,
) -> Result<(), ContactError> {
    let pair = Pair {
        wallet: wallet_id,
        identity,
        contact: identity,
    };
    let locks = identity_locks(wallet_id, identity);
    let _write = locks.write.lock().await;
    let own = Own::of(wallet, &pair).await?;
    let local_max_id = own.identity.public_keys().keys().max().copied();
    // Platform's key set, revision and balance, into the library's copy too:
    // the update is built on the cached revision.
    let current = net.refresh(wallet, identity).await?;
    let specs = upgrade_specs(current.public_keys().values(), local_max_id);
    if specs.is_empty() {
        return Ok(());
    }
    let index = own.index()?;
    let before = current.balance();
    cover(before, KEY_UPDATE_CREDITS)?;
    flow_keys(FlowKind::EnableDashPayKeys, &current, &[], |_| true)?;
    let lease = write_lease(
        session,
        wallet_id,
        &grant,
        &[FlowKind::EnableDashPayKeys],
        KEY_UPDATE_CREDITS,
    )
    .await?;
    let keys = {
        let deriving = lease
            .identity_signer(&[index])
            .map_err(PlatformError::from)?;
        let deriving = deriving.vault_signer().expect("not narrowed yet");
        derive_keys(deriving, index, &specs)
            .await
            .map_err(PlatformError::from)?
    };
    let added: Vec<KeyID> = keys.keys().copied().collect();
    let signer = flow_signer(&lease, index, &current, &added)?;
    charge(&lease, KEY_UPDATE_CREDITS)?;
    let written = leased(
        &lease,
        net.add_keys(wallet, identity, keys.into_values().collect(), &signer),
    )
    .await;
    drop(signer);
    // Platform's word on the keys, and the balance after.
    let after = net.refresh(wallet, identity).await;
    if let Err(e) = written {
        let keys = |a: &Identity| missing_dashpay_purposes(a.public_keys().values()).is_empty();
        if !after.as_ref().is_ok_and(keys) {
            return Err(e.into());
        }
        tracing::info!("the key upgrade landed despite: {e}");
    }
    match after {
        Ok(after) => note_charge(&identity, before, after.balance()),
        Err(e) => tracing::info!("keys added; the refresh after failed: {e}"),
    }
    Ok(())
}

#[cfg(test)]
#[path = "requests_tests.rs"]
pub(super) mod tests;
