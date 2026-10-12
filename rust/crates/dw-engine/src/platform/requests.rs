//! Contact requests (DP2-02, roadmap row 164): eligibility, send, accept,
//! ignore and unignore, and "Enable DashPay keys", behind the facade calls
//! of `contacts.rs`. Not part of the facade.
//!
//! Every write refuses what it can before it redeems a grant, in this
//! order: the identity and the other user (`eligibility`), the identity's
//! credits against the action's quote, then the grant or lease, whether the
//! vault holds a key its flow signs with, and last a scan's expiry. It signs
//! under a lease ([`write_lease`]) with exactly the keys its transition
//! needs ([`flow_keys`]), under a call permit charged the quote from the
//! lease's Credits budget ([`write_permit`]) whose signer is the only one it
//! uses, in the lease's dispatch scope ([`leased`]), and holds the
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
use super::keys_policy::{is_dashpay_recipient, key_path, missing_dashpay_purposes, upgrade_specs};
use super::lease::session::HeldKey;
use super::lease::{CallPermit, CallQuote, Lease};
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
///   that ends when the call drops it, and only while the vault holds one
///   of the flow's [`candidate_keys`]: the check and the redemption are one
///   section under the vault's record-write guard (dw-vault
///   `redeem_grant_holding_key`), so a wallet removal that came first is
///   `identity.keys_missing` with the grant unused. [`check_held_keys`]
///   asks the same before `ready`, which then runs after any vault wait.
///
/// A budget below `quote` is `platform.grant_exceeded`, none at all
/// `platform.needs_grant{Credits}`. `signs` is the identity at its index
/// whose keys the write signs with; a lease id's are checked by
/// [`flow_signer`] under the write's permit, whose quote goes back when it
/// refuses (nothing signed). `ready` is the
/// write's last check (a scan's expiry), right before a grant is redeemed:
/// the key check can wait on the vault.
async fn write_lease(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    grant: &str,
    (flows, quote): (&[FlowKind], u64),
    signs: (u32, &Identity),
    ready: impl FnOnce() -> Result<(), ContactError>,
) -> Result<Lease, ContactError> {
    let Some(lease) = session.lease_for(&wallet, grant) else {
        check_credit_cap(
            session,
            wallet,
            &NameAuthority::Grant(grant.to_owned()),
            quote,
        )
        .await?;
        check_held_keys(session, wallet, grant, flows[0], signs).await?;
        ready()?;
        let grants = [grant.to_owned()];
        let keys = candidate_keys(session, flows[0], signs);
        let lease = (session.begin_lease_holding(wallet, flows[0], &grants, keys)).await;
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
    ready()?;
    Ok(lease)
}

/// Refuses, with `grant` left unused, a write whose `flow` has no key the
/// vault holds ([`flow_keys`], `identity.keys_missing`): the candidate keys
/// are asked in turn, until one is held, through the grant without
/// redeeming it (dw-vault
/// `grant_identity_key_matches`). A vault error on the way (locked, the
/// grant gone) is that error rather than `keys_missing`. Advisory: the
/// redemption checks again ([`write_lease`]).
async fn check_held_keys(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    grant: &str,
    flow: FlowKind,
    (index, identity): (u32, &Identity),
) -> Result<(), ContactError> {
    let (vault, grant, identity) = (session.vault.clone(), grant.to_owned(), identity.clone());
    let checked = tokio::task::spawn_blocking(move || {
        let failed = std::cell::Cell::new(None);
        let held = |k: &IdentityPublicKey| {
            let Ok(path) = key_path(vault.network(), index, k.id()) else {
                return false;
            };
            vault
                .grant_identity_key_matches(&grant, &wallet.0, &path, k.data().as_slice())
                .unwrap_or_else(|e| {
                    failed.set(Some(e));
                    false
                })
        };
        match flow_keys(flow, &identity, &[], held) {
            Ok(_) => Ok(()),
            Err(e) => Err(failed.take().map_or(e, |e| PlatformError::from(e).into())),
        }
    })
    .await;
    checked.map_err(|e| PlatformError::from(crate::EngineError::from(e)))?
}

/// Runs `call`, a library write, under `lease`, in its dispatch scope.
pub(super) async fn leased<F: Future>(lease: &Lease, call: F) -> F::Output {
    lease.scope(call).await
}

/// The enabled ECDSA AUTHENTICATION keys at one of `levels`, in key-id
/// order, among which platform-wallet picks its signing key (the first its
/// signer's `can_sign_with` accepts).
fn auth_keys<'a>(
    identity: &'a Identity,
    levels: &'a [SecurityLevel],
) -> impl Iterator<Item = &'a IdentityPublicKey> {
    (identity.public_keys().values()).filter(|k| {
        k.purpose() == Purpose::AUTHENTICATION
            && levels.contains(&k.security_level())
            && k.key_type() == KeyType::ECDSA_SECP256K1
            && !k.is_disabled()
    })
}

/// The first of [`auth_keys`] that is `usable`.
fn first_auth_key(
    identity: &Identity,
    levels: &[SecurityLevel],
    usable: impl Fn(&IdentityPublicKey) -> bool,
) -> Option<KeyID> {
    auth_keys(identity, levels)
        .find(|k| usable(k))
        .map(|k| k.id())
}

/// The levels of the key `flow`'s transition signs with ([`flow_keys`]).
fn flow_levels(flow: FlowKind) -> &'static [SecurityLevel] {
    match flow {
        FlowKind::ContactRequest | FlowKind::Accept | FlowKind::AcceptAndPay => {
            &[SecurityLevel::HIGH, SecurityLevel::CRITICAL]
        }
        FlowKind::EnableDashPayKeys => &[SecurityLevel::MASTER],
        _ => &[],
    }
}

/// The keys [`flow_keys`] chooses among for `flow`, as (DIP-13 path, key
/// data), for a grant redeemed only while the vault holds one of them.
fn candidate_keys(
    session: &NetworkSession,
    flow: FlowKind,
    (index, identity): (u32, &Identity),
) -> Vec<HeldKey> {
    let network = session.vault.network();
    (auth_keys(identity, flow_levels(flow)))
        .filter_map(|k| Some((key_path(network, index, k.id()).ok()?, k.data().to_vec())))
        .collect()
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
/// key: `identity.keys_missing`, which a write checks before it redeems a
/// grant ([`check_held_keys`]).
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
            let key = first_auth_key(identity, flow_levels(flow), usable);
            Ok(vec![key.ok_or_else(missing)?])
        }
        FlowKind::EnableDashPayKeys => {
            let master = first_auth_key(identity, flow_levels(flow), usable);
            let master = master.ok_or_else(missing)?;
            Ok(std::iter::once(master)
                .chain(added.iter().copied())
                .collect())
        }
        _ => Err(PlatformError::GrantInvalid.into()),
    }
}

/// The call permit a library write runs under (E0-04 §2a.3, m4 §7 item
/// 39), charged `quote` from the lease's Credits budget: a lock's drain
/// waits for it, and its end decides the write's outcome by the signatures
/// its signers released. A write that fails or is refused before it signs
/// ends the permit with no signature, a definite `Cancelled`, and the quote
/// goes back; once a signature is out, the quote is kept whatever the call
/// returns (fail closed), so one lease id pays for one write's quote.
fn write_permit(lease: &Lease, label: &str, quote: u64) -> Result<CallPermit, PlatformError> {
    let quote = CallQuote {
        purpose: BudgetPurpose::Credits,
        amount: quote,
    };
    Ok(lease.call_permit(label, Some(quote))?)
}

/// The permit's identity signer for the identity at `index` (counting into
/// the permit), narrowed to [`flow_keys`] of the lease's flow, chosen among
/// the keys the vault holds.
fn flow_signer(
    permit: &CallPermit,
    index: u32,
    identity: &Identity,
    added: &[KeyID],
) -> Result<VaultIdentitySigner, ContactError> {
    let signer = permit
        .identity_signer(&[index])
        .map_err(PlatformError::from)?;
    let flow = permit.lease().flow();
    let keys = flow_keys(flow, identity, added, |k| signer.can_sign_with(k))?;
    Ok(signer.only_keys(keys))
}

/// What a contact-request write needs once its own checks pass, refused
/// before anything is redeemed where it can be: the identity's credits
/// against the quote, then the lease of one of `flows` (which checks the
/// vault holds the document key, then `ready`, a scan's expiry), the call
/// permit `label` charged the quote, its signer (the document key alone)
/// and the lease's contact crypto.
async fn prepare(
    net: &impl RequestNet,
    session: &Arc<NetworkSession>,
    own: &Own,
    (wallet, grant): (WalletId, &str),
    (flows, label): (&[FlowKind], &str),
    ready: impl FnOnce() -> Result<(), ContactError>,
) -> Result<(Lease, CallPermit, VaultIdentitySigner, VaultContactCrypto), ContactError> {
    let index = own.index()?;
    let id = own.identity.id();
    let balance = net.balance(id).await?.ok_or_else(not_found)?;
    cover(balance, CONTACT_REQUEST_CREDITS)?;
    let signs = (index, &own.identity);
    let cost = (flows, CONTACT_REQUEST_CREDITS);
    let lease = write_lease(session, wallet, grant, cost, signs, ready).await?;
    let permit = write_permit(&lease, label, CONTACT_REQUEST_CREDITS)?;
    let signer = flow_signer(&permit, index, &own.identity, &[])?;
    let crypto = lease.contact_crypto().map_err(PlatformError::from)?;
    Ok((lease, permit, signer, crypto))
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
    let (lease, permit, signer, crypto) = prepare(
        net,
        session,
        &own,
        (wallet_id, &grant),
        (&flows, "send_request"),
        ready,
    )
    .await?;
    // The last step that may refuse: the key goes out with the request, so
    // from here the scan is spent whatever happens (fail closed).
    let (proof, _spend) = match intent.auto_accept {
        Some(claim) => {
            let (proof, spend) = claim.proof_source(crate::events::unix_now())?;
            (proof, Some(spend))
        }
        None => (AutoAcceptProofSource::None, None),
    };
    let sent = leased(
        &lease,
        net.send(wallet, pair.identity, pair.contact, proof, &signer, &crypto),
    )
    .await;
    drop(signer);
    permit.finish(sent.is_ok());
    drop(lease);
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
    let (lease, permit, signer, crypto) = prepare(
        net,
        session,
        &own,
        (pair.wallet, &grant),
        (&flows, "accept_request"),
        || Ok(()),
    )
    .await?;
    let accepted = leased(&lease, net.accept(wallet, &request, &signer, &crypto)).await;
    drop(signer);
    permit.finish(accepted.is_ok());
    drop(lease);
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
    let lease = write_lease(
        session,
        wallet_id,
        &grant,
        (&[FlowKind::EnableDashPayKeys], KEY_UPDATE_CREDITS),
        (index, &current),
        || Ok(()),
    )
    .await?;
    let permit = write_permit(&lease, "enable_dashpay_keys", KEY_UPDATE_CREDITS)?;
    let keys = (permit.identity_signer(&[index]))
        .map_err(PlatformError::from)?
        .derive_public_keys(index, &specs)
        .await
        .map_err(PlatformError::from)?;
    let added: Vec<KeyID> = keys.keys().copied().collect();
    let signer = flow_signer(&permit, index, &current, &added)?;
    let written = leased(
        &lease,
        net.add_keys(wallet, identity, keys.into_values().collect(), &signer),
    )
    .await;
    drop(signer);
    permit.finish(written.is_ok());
    drop(lease);
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
