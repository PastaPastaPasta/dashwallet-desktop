//! Contacts: the read model, requests, ignore, private details, DashPay
//! keys, user links and scans (DP2-01…DP2-04).

use serde::{Deserialize, Serialize};

use super::dashpay::{BearerSecret, DashPay, stub};
use super::errors::ContactError;
use super::payments::PaymentLock;
use super::profile::Profile;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Eligibility {
    Ok,
    NoDashPayKeys,
    IsSelf,
    AlreadyContact,
    PendingOutgoing,
    PendingIncoming,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RequestOutcome {
    /// Sent; waiting for the other side.
    Pending { request: String },
    /// Both directions exist.
    Established { request: String },
}

/// A scanned or pasted contact, verified against Platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScannedContact {
    pub identity: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    /// Set when the payload carried a `dapk` auto-accept proof: an id for
    /// the proof, which the engine holds in memory for `send_request`.
    pub scan: Option<String>,
    pub relation: Relation,
    /// Verified only through the trusted fallback (§2.2 rule 2).
    pub unverified: bool,
}

#[expect(unused_variables, reason = "stubs until DP2-01…DP2-04")]
impl DashPay {
    pub fn contacts(
        &self,
        identity: String,
        q: ContactQuery,
    ) -> Result<ContactsPage, ContactError> {
        stub("DashPay.contacts")
    }

    pub fn contact(
        &self,
        identity: String,
        contact: String,
    ) -> Result<Option<ContactDetail>, ContactError> {
        stub("DashPay.contact")
    }

    pub fn pending_setup_count(&self) -> Result<u32, ContactError> {
        stub("DashPay.pending_setup_count")
    }

    pub async fn eligibility(
        &self,
        identity: String,
        contact: String,
    ) -> Result<Eligibility, ContactError> {
        stub("DashPay.eligibility")
    }

    pub async fn send_request(
        &self,
        identity: String,
        to: String,
        scan: Option<String>,
        grant: String,
    ) -> Result<RequestOutcome, ContactError> {
        stub("DashPay.send_request")
    }

    pub async fn accept_request(
        &self,
        identity: String,
        from: String,
        grant: String,
    ) -> Result<RequestOutcome, ContactError> {
        stub("DashPay.accept_request")
    }

    pub async fn ignore(&self, identity: String, contact: String) -> Result<(), ContactError> {
        stub("DashPay.ignore")
    }

    pub async fn unignore(&self, identity: String, contact: String) -> Result<(), ContactError> {
        stub("DashPay.unignore")
    }

    pub async fn set_private_details(
        &self,
        identity: String,
        contact: String,
        d: PrivateDetails,
        grant: Option<String>,
    ) -> Result<PublishState, ContactError> {
        stub("DashPay.set_private_details")
    }

    pub async fn enable_dashpay_keys(
        &self,
        identity: String,
        grant: String,
    ) -> Result<(), ContactError> {
        stub("DashPay.enable_dashpay_keys")
    }

    pub fn my_user_link(&self, identity: String) -> Result<String, ContactError> {
        stub("DashPay.my_user_link")
    }

    pub async fn verify_scanned(&self, text: BearerSecret) -> Result<ScannedContact, ContactError> {
        stub("DashPay.verify_scanned")
    }
}
