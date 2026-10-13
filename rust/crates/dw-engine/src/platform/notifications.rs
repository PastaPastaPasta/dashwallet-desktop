//! The bell: the `dp_events` journal read back (DP2-05). The changeset tap
//! (`journal.rs`) writes the rows, keeps the unread counts and hands the
//! OS notifier its news (`EngineEvent::DashPayNews`); here are the reads
//! and `mark_read`.

use std::sync::Arc;

use dpp::platform_value::string_encoding::Encoding;
use platform_wallet::PlatformWallet;
use serde::{Deserialize, Serialize};

use super::contacts::{ContactQuery, ContactSection, ContactSort, ContactSummary, Relation};
use super::contacts_model::identifier;
use super::dashpay::DashPay;
use super::errors::PlatformError;
use super::journal::event_of;
use crate::{NetworkSession, WalletId};

/// Events one `EngineEvent::DashPayNews` carries at most; its `more` counts
/// the rest.
pub const MAX_NEWS: usize = 32;

/// Events one `events` page holds at most.
const MAX_PAGE: u32 = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventPage {
    /// Incoming requests waiting for an answer.
    pub pending: Vec<ContactSummary>,
    /// Unread events, newest first.
    pub new: Vec<DashPayEvent>,
    /// Read events, newest first.
    pub earlier: Vec<DashPayEvent>,
    pub next_cursor: Option<u64>,
}

/// One `dp_events` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashPayEvent {
    pub id: u64,
    pub kind: EventKind,
    pub contact: Option<String>,
    /// Per kind: the txid of `PaymentReceived`, the contact-request id of
    /// the request kinds, the label of the name kinds.
    pub reference: Option<String>,
    pub at: u64,
    pub read_at: Option<u64>,
}

impl DashPayEvent {
    /// A journal row's event: an empty `contact` is `None`, `reference` as
    /// [`shown_reference`].
    pub(crate) fn journaled(
        id: u64,
        kind: EventKind,
        contact: &str,
        reference: &str,
        at: u64,
        read_at: Option<u64>,
    ) -> Self {
        Self {
            id,
            kind,
            contact: (!contact.is_empty()).then(|| contact.to_owned()),
            reference: shown_reference(kind, reference),
            at,
            read_at,
        }
    }
}

/// The journal kinds of §3.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    UsernameRegistered,
    ContestWon,
    ContestLost,
    ContestLocked,
    RequestReceived,
    RequestAccepted,
    ContactEstablished,
    PaymentReceived,
}

/// Whether an unread event of `kind` is news for the OS notifier. Not our
/// own acts: our accept (`ContactEstablished`) or a name we registered
/// (`UsernameRegistered`, which a restore journals unread when the name's
/// time is unknown, DEC-114 (4); iOS notifies contacts only). Not a
/// payment either: its `NewTransactions` notice announces it (DASHPAY F16).
pub(crate) fn notifies(kind: EventKind) -> bool {
    !matches!(
        kind,
        EventKind::ContactEstablished | EventKind::UsernameRegistered | EventKind::PaymentReceived
    )
}

/// The `reference` shown for a journal `ref`: a name kind's label without
/// its `@<stamp>` (m4 §5), `None` when empty.
fn shown_reference(kind: EventKind, raw: &str) -> Option<String> {
    let shown = match kind {
        EventKind::UsernameRegistered
        | EventKind::ContestWon
        | EventKind::ContestLost
        | EventKind::ContestLocked => raw.split('@').next().unwrap_or_default(),
        _ => raw,
    };
    (!shown.is_empty()).then(|| shown.to_owned())
}

impl DashPay {
    /// The bell (DP2-05): on the first page (`cursor` `None`) the incoming
    /// requests, then `limit` journal rows (1–200) below `cursor`, newest
    /// first, split into unread (`new`) and read (`earlier`). Rows of a
    /// sender the identity ignores are left out. `next_cursor` continues.
    pub async fn events(
        &self,
        identity: String,
        cursor: Option<u64>,
        limit: u32,
    ) -> Result<EventPage, PlatformError> {
        let (session, id) = (Arc::clone(&self.session), self.wallet_id());
        self.session
            .on_runtime(async move { Ok(session.event_page(id, identity, cursor, limit).await) })
            .await?
    }

    /// The identity's unread events, for the bell's badge. In memory, as
    /// the wallet's bring-up counted them; before that, 0, and the count
    /// starts, signalling `Platform{Events}` for each identity with unread
    /// events.
    pub fn unread_count(&self, identity: String) -> Result<u32, PlatformError> {
        let id = self.wallet_id();
        let _op = self.session.try_enter()?;
        let tap = self.session.live()?.tap;
        // Before the wallet check: a removal that finishes after it has
        // moved the epoch on (its `forget`), and the count is not started.
        let epoch = tap.epoch(id);
        self.session.require_wallet(&id)?;
        #[cfg(test)]
        self.session.test_pause_here("unread_count checked");
        let identity = canonical(&identity)?;
        Ok(tap.unread(id, &identity).unwrap_or_else(|| {
            tap.load_unread(id, epoch, &self.session.rt);
            0
        }))
    }

    /// Marks the identity's events with id ≤ `up_to` read: those of the
    /// wallet this call was admitted for, never a reimport's.
    pub async fn mark_read(&self, identity: String, up_to: u64) -> Result<(), PlatformError> {
        let id = self.wallet_id();
        self.on_wallet(move |session, _, wallet| async move {
            (session.mark_events_read(id, &wallet, identity, up_to, None)).await
        })
        .await
    }
}

/// The journal's spelling of an identity id: canonical Base58.
fn canonical(identity: &str) -> Result<String, PlatformError> {
    identifier(identity)
        .map(|i| i.to_string(Encoding::Base58))
        .map_err(platform_error)
}

/// The facade's error for a contacts read's: a contacts page fails only
/// with a `PlatformError`.
fn platform_error(e: super::errors::ContactError) -> PlatformError {
    match e {
        super::errors::ContactError::Platform(e) => e,
        other => PlatformError::Internal {
            detail: other.to_string(),
        },
    }
}

impl NetworkSession {
    async fn event_page(
        &self,
        id: WalletId,
        identity: String,
        cursor: Option<u64>,
        limit: u32,
    ) -> Result<EventPage, PlatformError> {
        let op = self.enter().await?;
        #[cfg(test)]
        self.test_pause("events").await;
        self.require_wallet(&id)?;
        let identity = canonical(&identity)?;
        // The live relations: Requests is Pending, the ignored senders are
        // in Hidden.
        let q = ContactQuery {
            sections: vec![ContactSection::Requests, ContactSection::Hidden],
            sort: ContactSort::DateAdded,
            text: None,
        };
        let page =
            (self.contacts_page_entered(&op, id, identity.clone(), q)).map_err(platform_error)?;
        let mut pending = Vec::new();
        let mut ignored = Vec::new();
        for section in page.sections {
            match section.section {
                ContactSection::Requests => pending = section.contacts,
                _ => ignored.extend(
                    (section.contacts.into_iter())
                        .filter(|c| c.relation == Relation::Ignored)
                        .map(|c| c.contact),
                ),
            }
        }
        let limit = limit.clamp(1, MAX_PAGE) as usize;
        let before = cursor.map(|c| i64::try_from(c).unwrap_or(i64::MAX));
        let wallet = id.to_string();
        let journal = self
            .appdb_op(move |db| db.journal_page(&wallet, &identity, before, limit, &ignored))
            .await?;
        let next_cursor = journal
            .more
            .then(|| journal.rows.last().map(|r| r.id as u64))
            .flatten();
        let (new, earlier) = (journal.rows.iter())
            .filter_map(event_of)
            .partition(|e| e.read_at.is_none());
        Ok(EventPage {
            pending: if cursor.is_none() {
                pending
            } else {
                Vec::new()
            },
            new,
            earlier,
            next_cursor,
        })
    }

    /// Marks the identity's events up to `up_to` read, with `contact` only
    /// that contact's. Signals `Platform{Events}` when one changed. The
    /// caller is inside the gate (`on_wallet`), which found `wallet`: the
    /// write lands under its fence ([`super::private_details::WriteFences`]),
    /// so a removal waits for it, and once `wallet` has left the manager
    /// (removed, or replaced by a reimport whose rows came back) it fails
    /// with `WalletNotFound` and marks nothing.
    pub(super) async fn mark_events_read(
        &self,
        id: WalletId,
        wallet: &PlatformWallet,
        identity: String,
        up_to: u64,
        contact: Option<String>,
    ) -> Result<(), PlatformError> {
        let identity = canonical(&identity)?;
        let contact = contact.as_deref().map(canonical).transpose()?;
        #[cfg(test)]
        self.test_pause("mark_read admitted").await;
        let fence = self.write_fence(id, wallet).await?;
        let tap = self.live()?.tap;
        (self.fenced_op(&fence, "mark_read", move |_| {
            tap.mark_read(id, &identity, up_to, contact.as_deref())
        }))
        .await?;
        Ok(())
    }

    /// Tests: [`Self::test_pause`] for a caller off the runtime.
    #[cfg(test)]
    fn test_pause_here(&self, at: &'static str) {
        if let Some(pause) = self.take_job_pause(at) {
            (pause.reached).store(true, std::sync::atomic::Ordering::SeqCst);
            self.rt.block_on(pause.release.notified());
        }
    }

    /// Tests: waits here while `job_pause` is set for `at`.
    #[cfg(test)]
    async fn test_pause(&self, at: &'static str) {
        if let Some(pause) = self.take_job_pause(at) {
            (pause.reached).store(true, std::sync::atomic::Ordering::SeqCst);
            pause.release.notified().await;
        }
    }
}
