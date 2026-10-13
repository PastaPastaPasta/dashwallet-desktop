//! Contacts: the read model, requests, ignore, private details, DashPay
//! keys, user links and scans (DP2-01…DP2-04).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::contacts_model::identifier;
use super::dashpay::{BearerSecret, DashPay};
use super::errors::{ContactError, PlatformError};
use super::payments::PaymentLock;
use super::private_details;
use super::profile::Profile;
use super::requests::{self, Pair, SdkRequests};

/// How a user relates to one of the wallet's identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    Stranger,
    Contact,
    PendingOutgoing,
    PendingIncoming,
    Ignored,
    IsSelf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactQuery {
    /// Empty means every section.
    pub sections: Vec<ContactSection>,
    pub sort: ContactSort,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContactSection {
    Requests,
    Contacts,
    Pending,
    Hidden,
}

/// Android's four orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContactSort {
    DisplayName,
    Username,
    DateAdded,
    LastActivity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactsPage {
    pub sections: Vec<ContactSectionPage>,
    /// A sync pass holds the wallet's state: the page is the last one read
    /// before it (possibly none yet). Show "updating", not "no contacts".
    pub updating: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactSectionPage {
    pub section: ContactSection,
    pub contacts: Vec<ContactSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactSummary {
    pub contact: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub alias: Option<String>,
    pub relation: Relation,
    pub hidden: bool,
    pub channel: ChannelState,
    pub since: Option<u64>,
    pub last_activity_at: Option<u64>,
    /// Verified only through the trusted fallback (§2.2 rule 2).
    pub unverified: bool,
}

/// The DIP-15 payment channel to a contact (F13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelState {
    NotEstablished,
    /// Waiting for an unlock to finish the contact crypto.
    SetupPending,
    Ready,
    /// `payment_channel_broken`.
    Broken,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactDetail {
    pub summary: ContactSummary,
    pub profile: Option<Profile>,
    pub private_details: PrivateDetails,
    pub publish_state: PublishState,
    pub request_sent_at: Option<u64>,
    pub request_received_at: Option<u64>,
    pub payment_lock: Option<PaymentLock>,
}

/// "Only visible to you" (F12).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PrivateDetails {
    pub alias: Option<String>,
    pub note: Option<String>,
    pub hidden: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublishState {
    Local,
    Published,
    DeferredUntilTwoContacts,
}

/// Whether `identity` may send `contact` a request (m4 §2.4), before any
/// prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Eligibility {
    Ok,
    /// The identity lacks its own DashPay keys: "Enable DashPay keys".
    NoDashPayKeys,
    IsSelf,
    AlreadyContact,
    PendingOutgoing,
    /// Their request is in: accept it instead.
    PendingIncoming,
    /// The contact has no key a request can be encrypted to (DP2-02).
    ContactCannotReceive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RequestOutcome {
    /// Sent; waiting for the other side.
    Pending { request: String },
    /// Both directions exist.
    Established { request: String },
}

/// A scanned or pasted contact. Verified against Platform means only that
/// `username` belongs to `identity` now: a QR is unsigned, so nothing shows
/// who made it, and a code's key stays a claim ([`ScannedCode`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScannedContact {
    pub identity: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    /// What was scanned, and what it claims beyond the name.
    pub code: ScannedCode,
    pub relation: Relation,
    /// The shown name or profile is verified only through the trusted
    /// fallback (§2.2 rule 2), as on a `UserHit`; says nothing of the key.
    pub unverified: bool,
}

/// The kind of a scanned code (m4 §2.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScannedCode {
    /// A `dashpay://user` link: the name and its identity, nothing more.
    UserLink,
    /// A DIP-15 code with an auto-accept key, held by the engine as `scan`
    /// for `send_request`. Its claim that the recipient accepts such a
    /// request on its own is unconfirmed: the key derives on a hardened
    /// path of the recipient's seed that is never published, so only the
    /// recipient can check it. A request sent with it is `Pending` like any
    /// other until the recipient accepts.
    AutoAcceptClaim { scan: String, expires_at: u64 },
}

impl DashPay {
    /// The Contacts screen for one of the wallet's identities, from memory
    /// (DP2-01, `contacts_model.rs`): the sections asked for, sorted and
    /// filtered by `text`. Re-query on `Platform{Contacts}`, `Identities`
    /// and `Payments`, and on the wallet's `HistoryChanged` (a block time
    /// moves `last_activity_at`).
    pub fn contacts(
        &self,
        identity: String,
        q: ContactQuery,
    ) -> Result<ContactsPage, ContactError> {
        self.session.contacts_page(self.wallet_id(), identity, q)
    }

    /// One contact of `identity` in any relation; `None` if unknown.
    pub fn contact(
        &self,
        identity: String,
        contact: String,
    ) -> Result<Option<ContactDetail>, ContactError> {
        self.session
            .contact_detail(self.wallet_id(), identity, contact)
    }

    /// Contacts of the wallet's identities that wait for an unlock to
    /// finish their channel or accept their request.
    pub fn pending_setup_count(&self) -> Result<u32, ContactError> {
        self.session.pending_setup(self.wallet_id())
    }

    /// The identity's `dashpay://user?id=&username=` link with its main
    /// name, as iOS writes it (DP2-03, `users.rs`). An identity without a
    /// name has none: `contact.no_username`.
    pub fn my_user_link(&self, identity: String) -> Result<String, ContactError> {
        self.session.my_user_link_of(self.wallet_id(), &identity)
    }

    /// A scanned or pasted `dashpay://user` link or DIP-15
    /// `dash:?du=&dapk=` code, verified against Platform (DP2-03). A `dapk`
    /// key stays in the engine; `scan` names it for `send_request`.
    pub async fn verify_scanned(&self, text: BearerSecret) -> Result<ScannedContact, ContactError> {
        let (session, id) = (Arc::clone(&self.session), self.wallet_id());
        self.session
            .on_runtime(async move { Ok(session.verify_scanned_of(id, text.expose()).await) })
            .await
            .map_err(PlatformError::from)?
    }
}

impl DashPay {
    /// Whether `identity` may send `contact` a request, before any prompt
    /// or grant (DP2-02, `requests.rs`): its own state first (`IsSelf`,
    /// `AlreadyContact`, `PendingOutgoing`, `PendingIncoming`, then
    /// `NoDashPayKeys` for its own keys), then the contact's keys on
    /// Platform (`ContactCannotReceive`; an unknown contact is
    /// `identity.not_found`).
    pub async fn eligibility(
        &self,
        identity: String,
        contact: String,
    ) -> Result<Eligibility, ContactError> {
        let wallet_id = self.wallet_id();
        self.on_wallet(move |_, manager, wallet| async move {
            let pair = Pair::parse(wallet_id, &identity, &contact)?;
            requests::eligibility(&SdkRequests(manager.sdk_arc()), &wallet, pair).await
        })
        .await
    }

    /// Sends `identity`'s contact request to `to` (m4 §2.4). Refused first,
    /// leaving the grant and the scan for a retry: an ineligible target
    /// (`contact.ineligible{reason}`), a scan whose name was shown on the
    /// fallback's word while writes wait for SPV
    /// (`platform.context_unavailable`), credits below the quote
    /// (`platform.insufficient_credits`), and a grant or lease that cannot
    /// cover it. `grant` is a `PlatformOp` grant or a `ContactRequest`
    /// lease; the request is signed with the document key alone.
    pub async fn send_request(
        &self,
        identity: String,
        to: String,
        scan: Option<String>,
        grant: String,
    ) -> Result<RequestOutcome, ContactError> {
        let wallet_id = self.wallet_id();
        self.on_wallet(move |session, manager, wallet| async move {
            let net = SdkRequests(manager.sdk_arc());
            requests::send(
                &net,
                &session,
                &wallet,
                wallet_id,
                (identity, to, scan),
                grant,
            )
            .await
        })
        .await
    }

    /// Accepts `from`'s request: `Established`. `grant` is a `PlatformOp`
    /// grant or an `Accept` or `AcceptAndPay` lease, whose Spend part the
    /// payment's `prepare` uses next.
    pub async fn accept_request(
        &self,
        identity: String,
        from: String,
        grant: String,
    ) -> Result<RequestOutcome, ContactError> {
        let wallet_id = self.wallet_id();
        self.on_wallet(move |session, manager, wallet| async move {
            let pair = Pair::parse(wallet_id, &identity, &from)?;
            let net = SdkRequests(manager.sdk_arc());
            requests::accept(&net, &session, &wallet, pair, grant).await
        })
        .await
    }

    /// Hides `contact`'s requests from `identity`, locally (m4 §2.4).
    pub async fn ignore(&self, identity: String, contact: String) -> Result<(), ContactError> {
        let wallet_id = self.wallet_id();
        self.on_wallet(move |_, _, wallet| async move {
            let pair = Pair::parse(wallet_id, &identity, &contact)?;
            requests::set_ignored(&wallet, pair, true).await
        })
        .await
    }

    /// Undoes `ignore`; a contact not ignored is no error.
    pub async fn unignore(&self, identity: String, contact: String) -> Result<(), ContactError> {
        let wallet_id = self.wallet_id();
        self.on_wallet(move |_, _, wallet| async move {
            let pair = Pair::parse(wallet_id, &identity, &contact)?;
            requests::set_ignored(&wallet, pair, false).await
        })
        .await
    }

    /// Saves `identity`'s alias, note and hidden flag for `contact`, an
    /// established contact, and with `grant` (a `PlatformOp` grant or a
    /// `PrivateDetails` lease) publishes them as its encrypted `contactInfo`
    /// (DP2-04, `private_details.rs`). While the identity has fewer than two
    /// contacts they are only saved (`DeferredUntilTwoContacts`). Without a
    /// grant, details Platform already holds for the contact cannot change
    /// (`platform.needs_grant`): DashPay's next pass would put them back.
    pub async fn set_private_details(
        &self,
        identity: String,
        contact: String,
        d: PrivateDetails,
        grant: Option<String>,
    ) -> Result<PublishState, ContactError> {
        let wallet_id = self.wallet_id();
        self.on_wallet(move |session, manager, wallet| async move {
            let pair = Pair::parse(wallet_id, &identity, &contact)?;
            let net = SdkRequests(manager.sdk_arc());
            private_details::set(&net, &session, &wallet, pair, d, grant).await
        })
        .await
    }

    /// Adds the DashPay keys `identity` lacks on Platform (F9); nothing
    /// missing is `Ok` with nothing sent. `grant` is a `PlatformOp` grant or
    /// an `EnableDashPayKeys` lease; the update is signed by the master key
    /// and the added keys alone.
    pub async fn enable_dashpay_keys(
        &self,
        identity: String,
        grant: String,
    ) -> Result<(), ContactError> {
        let wallet_id = self.wallet_id();
        self.on_wallet(move |session, manager, wallet| async move {
            let identity = identifier(&identity)?;
            let net = SdkRequests(manager.sdk_arc());
            requests::enable_keys(&net, &session, &wallet, wallet_id, identity, grant).await
        })
        .await
    }
}
