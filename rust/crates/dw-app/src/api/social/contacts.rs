//! Contact records: the read model, requests, private details, scans.

use dw_engine::platform::{
    ChannelState, ContactDetail, ContactQuery, ContactSection, ContactSectionPage, ContactSort,
    ContactSummary, ContactsPage, PrivateDetails, PublishState, RequestOutcome, ScannedCode,
    ScannedContact,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::payments::PaymentLockInfo;
use super::profile::DashPayProfile;
use crate::dto::mirror_enum;
use crate::dto::platform::UserRelation;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ContactSectionName {
    Requests,
    Contacts,
    Pending,
    Hidden,
}

mirror_enum!(ContactSection => ContactSectionName { Requests, Contacts, Pending, Hidden });
mirror_enum!(ContactSectionName => ContactSection { Requests, Contacts, Pending, Hidden });

/// Android's four orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ContactSortName {
    DisplayName,
    Username,
    DateAdded,
    LastActivity,
}

mirror_enum!(ContactSort => ContactSortName { DisplayName, Username, DateAdded, LastActivity });
mirror_enum!(ContactSortName => ContactSort { DisplayName, Username, DateAdded, LastActivity });

/// The DIP-15 payment channel to a contact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ChannelStateName {
    NotEstablished,
    /// Waiting for an unlock to finish the contact crypto.
    SetupPending,
    Ready,
    Broken,
}

mirror_enum!(ChannelState => ChannelStateName { NotEstablished, SetupPending, Ready, Broken });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PublishStateName {
    Local,
    Published,
    DeferredUntilTwoContacts,
}

mirror_enum!(PublishState => PublishStateName { Local, Published, DeferredUntilTwoContacts });

/// Which contacts `dashpayContacts` lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ContactQueryArgs {
    /// Empty means every section.
    pub sections: Vec<ContactSectionName>,
    pub sort: ContactSortName,
    pub text: Option<String>,
}

impl From<ContactQueryArgs> for ContactQuery {
    fn from(q: ContactQueryArgs) -> Self {
        let ContactQueryArgs {
            sections,
            sort,
            text,
        } = q;
        ContactQuery {
            sections: sections.into_iter().map(Into::into).collect(),
            sort: sort.into(),
            text,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ContactSummaryInfo {
    /// The contact's identity, Base58.
    pub contact: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub alias: Option<String>,
    pub relation: UserRelation,
    pub hidden: bool,
    pub channel: ChannelStateName,
    /// UNIX time of the contact.
    pub since: Option<u64>,
    pub last_activity_at: Option<u64>,
    /// Verified only through the trusted fallback.
    pub unverified: bool,
}

impl From<ContactSummary> for ContactSummaryInfo {
    fn from(c: ContactSummary) -> Self {
        let ContactSummary {
            contact,
            username,
            display_name,
            alias,
            relation,
            hidden,
            channel,
            since,
            last_activity_at,
            unverified,
        } = c;
        ContactSummaryInfo {
            contact,
            username,
            display_name,
            alias,
            relation: relation.into(),
            hidden,
            channel: channel.into(),
            since,
            last_activity_at,
            unverified,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ContactSectionRows {
    pub section: ContactSectionName,
    pub contacts: Vec<ContactSummaryInfo>,
}

impl From<ContactSectionPage> for ContactSectionRows {
    fn from(s: ContactSectionPage) -> Self {
        let ContactSectionPage { section, contacts } = s;
        ContactSectionRows {
            section: section.into(),
            contacts: contacts.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ContactList {
    pub sections: Vec<ContactSectionRows>,
    /// A sync pass holds the wallet's state: the page is the last one read
    /// before it (possibly none yet). Show "updating", not "no contacts".
    pub updating: bool,
}

impl From<ContactsPage> for ContactList {
    fn from(p: ContactsPage) -> Self {
        let ContactsPage { sections, updating } = p;
        ContactList {
            sections: sections.into_iter().map(Into::into).collect(),
            updating,
        }
    }
}

/// "Only visible to you": a contact's alias, note and hidden flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PrivateContactDetails {
    pub alias: Option<String>,
    pub note: Option<String>,
    pub hidden: bool,
}

impl From<PrivateDetails> for PrivateContactDetails {
    fn from(d: PrivateDetails) -> Self {
        let PrivateDetails {
            alias,
            note,
            hidden,
        } = d;
        PrivateContactDetails {
            alias,
            note,
            hidden,
        }
    }
}

impl From<PrivateContactDetails> for PrivateDetails {
    fn from(d: PrivateContactDetails) -> Self {
        let PrivateContactDetails {
            alias,
            note,
            hidden,
        } = d;
        PrivateDetails {
            alias,
            note,
            hidden,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ContactDetailInfo {
    pub summary: ContactSummaryInfo,
    pub profile: Option<DashPayProfile>,
    pub private_details: PrivateContactDetails,
    pub publish_state: PublishStateName,
    pub request_sent_at: Option<u64>,
    pub request_received_at: Option<u64>,
    pub payment_lock: Option<PaymentLockInfo>,
}

impl From<ContactDetail> for ContactDetailInfo {
    fn from(d: ContactDetail) -> Self {
        let ContactDetail {
            summary,
            profile,
            private_details,
            publish_state,
            request_sent_at,
            request_received_at,
            payment_lock,
        } = d;
        ContactDetailInfo {
            summary: summary.into(),
            profile: profile.map(Into::into),
            private_details: private_details.into(),
            publish_state: publish_state.into(),
            request_sent_at,
            request_received_at,
            payment_lock: payment_lock.map(Into::into),
        }
    }
}

/// What a contact request or its acceptance did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ContactRequestOutcome {
    /// Sent; waiting for the other side.
    Pending { request: String },
    /// Both directions exist.
    Established { request: String },
}

impl From<RequestOutcome> for ContactRequestOutcome {
    fn from(o: RequestOutcome) -> Self {
        match o {
            RequestOutcome::Pending { request } => ContactRequestOutcome::Pending { request },
            RequestOutcome::Established { request } => {
                ContactRequestOutcome::Established { request }
            }
        }
    }
}

/// The kind of a scanned code (m4 §2.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ScannedCodeInfo {
    /// A `dashpay://user` link: the name and its identity, nothing more.
    UserLink,
    /// A DIP-15 code with an auto-accept key: `scan` is an id the engine
    /// holds in memory until `expiresAt` (UNIX seconds); pass it as `scan`
    /// to `dashpaySendRequest`. Its claim that the recipient accepts on its
    /// own is unconfirmed: a request sent with it is pending like any other.
    AutoAcceptClaim { scan: String, expires_at: u64 },
}

impl From<ScannedCode> for ScannedCodeInfo {
    fn from(c: ScannedCode) -> Self {
        match c {
            ScannedCode::UserLink => Self::UserLink,
            ScannedCode::AutoAcceptClaim { scan, expires_at } => {
                Self::AutoAcceptClaim { scan, expires_at }
            }
        }
    }
}

/// A scanned or pasted contact, verified against Platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ScannedContactInfo {
    pub identity: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    /// What was scanned, and what it claims beyond the name.
    pub code: ScannedCodeInfo,
    pub relation: UserRelation,
    /// Verified only through the trusted fallback.
    pub unverified: bool,
}

impl From<ScannedContact> for ScannedContactInfo {
    fn from(s: ScannedContact) -> Self {
        let ScannedContact {
            identity,
            username,
            display_name,
            code,
            relation,
            unverified,
        } = s;
        ScannedContactInfo {
            identity,
            username,
            display_name,
            code: code.into(),
            relation: relation.into(),
            unverified,
        }
    }
}
