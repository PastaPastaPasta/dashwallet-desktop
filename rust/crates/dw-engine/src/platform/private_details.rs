//! Private details (DP2-04, roadmap row 166): a contact's alias, note and
//! hidden flag, kept by platform-wallet with the contact and published as
//! the identity's encrypted DIP-15 `contactInfo` document, behind
//! `DashPay::set_private_details`. Not part of the facade.
//!
//! What Platform holds is recorded as a digest per contact
//! (`dp_contact_info`, no plaintext): the details read as `Published` when
//! their digest equals it, else `Local`, and `DeferredUntilTwoContacts`
//! while the identity has fewer than two contacts (DIP-15: a single
//! `contactInfo` is trivially linkable to the pair's contact requests). The
//! digest is written by a publish that landed, and by a read of the
//! identity's documents: a write's read-back, and [`NetworkSession::settle_contact_infos`]
//! after a bring-up or an unlock, which is how a restore gets its details
//! and their state back.
//!
//! The library's DashPay pass applies Platform's `contactInfo` over local
//! details on every pass ("remote wins"), so a local-only edit of a contact
//! Platform holds details for would silently go back: it needs a grant
//! (`platform.needs_grant`, m4 §7 item 42). With no digest recorded, the
//! edit reads Platform first. A write that failed once signed keeps the
//! edit until a read finds whether it landed.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use dash_sdk::platform::{DocumentQuery, FetchMany};
use dashcore::hashes::{Hash, sha256};
use dpp::document::{Document, DocumentV0Getters};
use dpp::identity::Identity;
use dpp::identity::accessors::IdentityGettersV0;
use dpp::platform_value::Value;
use dpp::prelude::Identifier;
use dw_appdb::{AppDb, AppDbError};
use platform_wallet::wallet::identity::crypto::contact_info::{
    ContactInfoPrivateData, MAX_PLAINTEXT_LEN, decode_private_data, encode_private_data,
};
use platform_wallet::{ContactInfoPublishOutcome, EstablishedContact, PlatformWallet};

pub(super) use super::contact_names::BoxedFuture;
use super::contacts::{Eligibility, PrivateDetails, PublishState};
use super::errors::{ContactError, PlatformError};
use super::flows::{BudgetPurpose, DOCUMENT_10K_CREDITS, FlowKind};
use super::identity_writes::identity_locks;
use super::keys_policy::{key_path, missing_dashpay_purposes};
use super::lease::FlowOutcome;
use super::registration_flow::b58;
use super::requests::{Pair, RequestNet, leased, not_found, prepare};
#[cfg(test)]
use super::runtime::guard;
use super::signers::VaultContactCrypto;
use crate::events::unix_now;
use crate::{NetworkSession, PlatformChange, WalletId};

/// `contactInfo` documents per page of a read.
const PAGE: u32 = 100;
/// The most documents one identity's read takes; more is a failed read.
/// An identity has one per contact it published details for.
const MAX_DOCUMENTS: usize = 1000;
/// How long one identity's read may take before it counts as failed, as a
/// contact's username fetch (`contact_names.rs`).
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// After a write's read-back failed: when the details are read again, and
/// how many times, before the next bring-up or unlock does it.
const RESETTLE_AFTER: Duration = if cfg!(test) {
    Duration::from_millis(500)
} else {
    Duration::from_secs(60)
};
const RESETTLE_TRIES: u32 = 5;
/// Domain of [`digest`].
const DIGEST_DOMAIN: &[u8] = b"dw.contact_info.v1";

/// One `contactInfo` document as Platform holds it, still sealed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InfoDocument {
    /// `rootEncryptionKeyIndex`: the owner's key the AES keys derive under.
    pub(super) root: u32,
    /// `derivationEncryptionKeyIndex`.
    pub(super) derivation: u32,
    pub(super) enc_to_user_id: [u8; 32],
    pub(super) private_data: Vec<u8>,
    /// `$updatedAt`, UNIX ms.
    pub(super) updated_at: u64,
}

/// Where an identity's `contactInfo` documents come from: the SDK, or a
/// test's script.
pub(super) trait InfoSource: Send + Sync {
    /// Every `contactInfo` document `owner` holds, oldest update first.
    fn documents(
        &self,
        owner: Identifier,
    ) -> BoxedFuture<'_, Result<Vec<InfoDocument>, PlatformError>>;
}

/// `contactInfo` through the SDK: `$ownerId == owner` ordered by
/// `$updatedAt` (load-bearing, as in platform-wallet's own fetch: drive
/// answers a bare equality on that index with a proof of absence), paged,
/// at most [`MAX_DOCUMENTS`].
struct SdkInfos(Arc<dash_sdk::Sdk>);

impl InfoSource for SdkInfos {
    fn documents(
        &self,
        owner: Identifier,
    ) -> BoxedFuture<'_, Result<Vec<InfoDocument>, PlatformError>> {
        use dash_sdk::dapi_grpc::platform::v0::get_documents_request::get_documents_request_v0::Start;
        use dash_sdk::drive::query::{OrderClause, WhereClause, WhereOperator};
        Box::pin(async move {
            let contract = super::registration_flow::dashpay_contract()?;
            let mut out = Vec::new();
            let mut start = None;
            loop {
                let mut query = DocumentQuery::new(Arc::clone(&contract), "contactInfo")
                    .map_err(|e| PlatformError::from(dash_sdk::Error::from(e)))?
                    .with_where(WhereClause {
                        field: "$ownerId".into(),
                        operator: WhereOperator::Equal,
                        value: Value::Identifier(owner.to_buffer()),
                    })
                    .with_order_by(OrderClause {
                        field: "$updatedAt".into(),
                        ascending: true,
                    });
                query.limit = PAGE;
                query.start = start.take();
                let page = Document::fetch_many(&self.0, query).await?;
                let (len, last) = (page.len(), page.keys().last().copied());
                out.extend(
                    page.into_values()
                        .flatten()
                        .filter_map(|d| document(&owner, &d)),
                );
                if out.len() > MAX_DOCUMENTS {
                    return Err(PlatformError::Internal {
                        detail: format!("more than {MAX_DOCUMENTS} contactInfo documents"),
                    });
                }
                match last {
                    Some(id) if len >= PAGE as usize => {
                        start = Some(Start::StartAfter(id.to_buffer().to_vec()));
                    }
                    _ => return Ok(out),
                }
            }
        })
    }
}

/// A fetched document's fields; `None` (logged) for one that lacks them.
fn document(owner: &Identifier, d: &Document) -> Option<InfoDocument> {
    let p = d.properties();
    let int = |f: &str| p.get(f).and_then(|v| v.to_integer::<u32>().ok());
    let bytes = |f: &str| p.get(f).and_then(|v| v.to_binary_bytes().ok());
    let found = (|| {
        Some(InfoDocument {
            root: int("rootEncryptionKeyIndex")?,
            derivation: int("derivationEncryptionKeyIndex")?,
            enc_to_user_id: bytes("encToUserId")?.try_into().ok()?,
            private_data: bytes("privateData")?,
            updated_at: d.updated_at().unwrap_or(0),
        })
    })();
    if found.is_none() {
        tracing::warn!(owner = %owner, doc = %d.id(), "a contactInfo document without its fields");
    }
    found
}

/// The details as the library holds and DIP-15 encodes them; no accepted
/// accounts (desktop uses account 0 only, m4 §7 item 42).
impl From<&PrivateDetails> for ContactInfoPrivateData {
    fn from(d: &PrivateDetails) -> Self {
        Self {
            alias_name: d.alias.clone(),
            note: d.note.clone(),
            display_hidden: d.hidden,
            accepted_accounts: Vec::new(),
        }
    }
}

impl From<ContactInfoPrivateData> for PrivateDetails {
    fn from(d: ContactInfoPrivateData) -> Self {
        Self {
            alias: d.alias_name,
            note: d.note,
            hidden: d.display_hidden,
        }
    }
}

impl From<&EstablishedContact> for PrivateDetails {
    fn from(c: &EstablishedContact) -> Self {
        Self {
            alias: c.alias.clone(),
            note: c.note.clone(),
            hidden: c.is_hidden,
        }
    }
}

fn encoded(d: &PrivateDetails) -> Vec<u8> {
    encode_private_data(&d.into())
}

/// What `dp_contact_info` records of details: SHA-256 over a domain and
/// their DIP-15 encoding.
pub(super) fn digest(d: &PrivateDetails) -> [u8; 32] {
    let mut bytes = DIGEST_DOMAIN.to_vec();
    bytes.extend(encoded(d));
    sha256::Hash::hash(&bytes).to_byte_array()
}

/// Trimmed; a blank alias or note is none.
fn normalized(d: PrivateDetails) -> PrivateDetails {
    let text = |t: Option<String>| t.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
    PrivateDetails {
        alias: text(d.alias),
        note: text(d.note),
        hidden: d.hidden,
    }
}

/// The state of details `shown` given `contacts` and the `recorded` digest.
pub(super) fn publish_state(
    contacts: usize,
    shown: &PrivateDetails,
    recorded: Option<[u8; 32]>,
) -> PublishState {
    if contacts < 2 {
        PublishState::DeferredUntilTwoContacts
    } else if recorded == Some(digest(shown)) {
        PublishState::Published
    } else {
        PublishState::Local
    }
}

/// Opens `owner`'s documents (identity `index`) with `crypto`: what
/// Platform holds for each contact, the latest update winning. A signer
/// that cannot open them (a locked vault) fails the read; a document that
/// does not open with the wallet's keys (another client's, a corrupt one),
/// or whose plaintext does not decode, is skipped, as the library skips it.
async fn read_platform(
    source: &dyn InfoSource,
    crypto: &VaultContactCrypto,
    (network, index, owner): (key_wallet::Network, u32, Identifier),
) -> Result<BTreeMap<Identifier, PrivateDetails>, PlatformError> {
    let documents = tokio::time::timeout(READ_TIMEOUT, source.documents(owner))
        .await
        .map_err(|_| PlatformError::Timeout)??;
    let mut found = BTreeMap::new();
    for doc in documents {
        let root = key_path(network, index, doc.root).ok();
        let opened = match root {
            Some(root) => crypto.contact_info_open_or_skip(
                &root,
                doc.derivation,
                &doc.enc_to_user_id,
                &doc.private_data,
            )?,
            None => None,
        };
        let Some(opened) = opened else {
            tracing::warn!(owner = %owner, "a contactInfo that does not open with the wallet's keys");
            continue;
        };
        match decode_private_data(&opened.private_data) {
            Ok(data) => {
                found.insert(Identifier::from(opened.contact_id), data.into());
            }
            Err(e) => {
                tracing::warn!(owner = %owner, error = %e, "a contactInfo that does not decode")
            }
        }
    }
    Ok(found)
}

/// The wallet's identity and its contact as platform-wallet holds them.
struct Mine {
    index: Option<u32>,
    identity: Identity,
    contacts: usize,
    /// The contact's details; `None` when it is no established contact.
    details: Option<PrivateDetails>,
}

impl Mine {
    async fn of(wallet: &PlatformWallet, pair: &Pair) -> Result<Self, ContactError> {
        let state = wallet.state().await;
        let m = (state.identity_manager.managed_identity(&pair.identity))
            .filter(|m| m.wallet_id == Some(wallet.wallet_id()))
            .ok_or_else(not_found)?;
        let established = m.dashpay().established_contacts();
        Ok(Self {
            index: m.identity_index,
            identity: m.identity.clone(),
            contacts: established.len(),
            details: established.get(&pair.contact).map(PrivateDetails::from),
        })
    }
}

/// Saves `d` as `pair.identity`'s details of `pair.contact` in the library
/// (its changeset tap signals `Contacts`), as its own DashPay pass applies
/// a `contactInfo`. A contact no longer established is left alone.
async fn save_local(
    wallet: &PlatformWallet,
    pair: &Pair,
    d: &PrivateDetails,
) -> Result<(), PlatformError> {
    let mut state = wallet.state_mut().await;
    let Some(managed) = (state.identity_manager).managed_identity_mut(&pair.identity) else {
        return Ok(());
    };
    managed
        .set_contact_metadata(&pair.contact, d.into(), wallet.persister())
        .map_err(|e| PlatformError::Storage {
            detail: format!("private details not saved: {e}"),
        })?;
    Ok(())
}

impl NetworkSession {
    fn info_source(&self) -> Result<Arc<dyn InfoSource>, PlatformError> {
        #[cfg(test)]
        if let Some(source) = guard(&self.platform.contacts.info_source).clone() {
            return Ok(source);
        }
        let manager = self.manager()?;
        Ok(Arc::new(SdkInfos(manager.sdk_arc())))
    }

    /// `f` on app.sqlite with `pair` as `dp_contact_info` keys it.
    async fn contact_info_op<T: Send + 'static>(
        &self,
        pair: &Pair,
        f: impl FnOnce(&AppDb, &str, &str, &str) -> Result<T, AppDbError> + Send + 'static,
    ) -> Result<T, PlatformError> {
        let (wallet, identity, contact) = (
            pair.wallet.to_string(),
            b58(&pair.identity),
            b58(&pair.contact),
        );
        Ok(self
            .appdb_op(move |db| f(db, &wallet, &identity, &contact))
            .await?)
    }

    /// The digest `dp_contact_info` holds for `pair`.
    pub(super) async fn recorded_digest(
        &self,
        pair: &Pair,
    ) -> Result<Option<[u8; 32]>, PlatformError> {
        self.contact_info_op(pair, |db, w, i, c| db.contact_info_digest(w, i, c))
            .await
    }

    /// Records that Platform holds `d` for `pair`. A change signals
    /// `Contacts`: the details' `publish_state` moved.
    async fn record_digest(&self, pair: &Pair, d: &PrivateDetails) -> Result<(), PlatformError> {
        let digest = digest(d);
        let changed = self
            .contact_info_op(pair, move |db, w, i, c| {
                db.set_contact_info_digest(w, i, c, &digest, unix_now())
            })
            .await?;
        self.digest_changed(pair, changed);
        Ok(())
    }

    /// Forgets what was recorded for `pair`: Platform holds nothing for it.
    async fn forget_digest(&self, pair: &Pair) -> Result<(), PlatformError> {
        let changed = self
            .contact_info_op(pair, |db, w, i, c| db.forget_contact_info_digest(w, i, c))
            .await?;
        self.digest_changed(pair, changed);
        Ok(())
    }

    fn digest_changed(&self, pair: &Pair, changed: bool) {
        if changed {
            let identity = b58(&pair.identity);
            (self.hub.pump).mark_platform(pair.wallet, [PlatformChange::Contacts { identity }]);
        }
    }

    /// Reads what Platform holds of `owner`'s details (identity `index`)
    /// and makes the wallet agree, under the identity's write lock: each
    /// established contact gets Platform's details (as the library's own
    /// pass applies them) and its digest recorded; a contact Platform holds
    /// nothing for loses its record. A failed read changes nothing. The
    /// lock is held across the read (at most [`READ_TIMEOUT`]), so a write
    /// of the identity cannot land between the read and what it applies.
    async fn settle_identity(
        &self,
        wallet: &PlatformWallet,
        at: (WalletId, u32, Identifier),
        crypto: &VaultContactCrypto,
    ) -> Result<(), PlatformError> {
        let locks = identity_locks(at.0, at.2);
        let _write = locks.write.lock().await;
        self.settle_locked(wallet, at, crypto).await
    }

    /// [`Self::settle_identity`] under the write lock its caller holds.
    async fn settle_locked(
        &self,
        wallet: &PlatformWallet,
        (id, index, owner): (WalletId, u32, Identifier),
        crypto: &VaultContactCrypto,
    ) -> Result<(), PlatformError> {
        let source = self.info_source()?;
        let found = read_platform(&*source, crypto, (self.vault.network(), index, owner)).await?;
        let established: Vec<(Identifier, PrivateDetails)> = {
            let state = wallet.state().await;
            let Some(m) = state.identity_manager.managed_identity(&owner) else {
                return Ok(());
            };
            (m.dashpay().established_contacts().iter())
                .map(|(contact, c)| (*contact, c.into()))
                .collect()
        };
        for (contact, shown) in established {
            let pair = Pair {
                wallet: id,
                identity: owner,
                contact,
            };
            match found.get(&contact) {
                Some(held) => {
                    if *held != shown {
                        save_local(wallet, &pair, held).await?;
                    }
                    self.record_digest(&pair, held).await?;
                }
                None => self.forget_digest(&pair).await?,
            }
        }
        Ok(())
    }

    /// After a bring-up or an unlock: settles the private details of every
    /// identity of wallet `id` the wallet holds an index for, with the
    /// vault's prompt-free crypto ([`Self::settle_identity`]). `only` limits
    /// it to one identity. Errors are logged: the next one tries again.
    pub(super) async fn settle_contact_infos(
        &self,
        wallet: &PlatformWallet,
        id: WalletId,
        crypto: &VaultContactCrypto,
        only: Option<Identifier>,
    ) -> bool {
        let owners: Vec<(Identifier, u32)> = {
            let state = wallet.state().await;
            (state.identity_manager.managed_identities())
                .filter(|m| m.wallet_id == Some(wallet.wallet_id()))
                .filter_map(|m| Some((m.id(), m.identity_index?)))
                .filter(|(owner, _)| only.is_none_or(|o| o == *owner))
                .collect()
        };
        let mut settled = true;
        for (owner, index) in owners {
            if let Err(e) = self
                .settle_identity(wallet, (id, index, owner), crypto)
                .await
            {
                tracing::info!(wallet_id = %id, identity = %b58(&owner), error = %e, "private details not settled");
                settled = false;
            }
        }
        settled
    }

    /// After a write that failed once signed and whose read-back did not
    /// find it: settles `owner` again every [`RESETTLE_AFTER`], up to
    /// [`RESETTLE_TRIES`] times (a try while the vault needs a prompt reads
    /// nothing), so details that landed late become `Published` without
    /// another write, and details that did not are put back. Session close
    /// cancels it.
    fn resettle_later(self: &Arc<Self>, id: WalletId, owner: Identifier) {
        let weak = Arc::downgrade(self);
        let task = async move {
            for _ in 0..RESETTLE_TRIES {
                tokio::time::sleep(RESETTLE_AFTER).await;
                let Some(this) = weak.upgrade() else { return };
                let Some(crypto) = this.prompt_free_crypto(id).await else {
                    continue;
                };
                let Ok(manager) = this.manager() else { return };
                let Some(wallet) = manager.get_wallet(&id.0).await else {
                    return;
                };
                if this
                    .settle_contact_infos(&wallet, id, &crypto, Some(owner))
                    .await
                {
                    return;
                }
            }
        };
        self.platform.contacts.spawn(&self.rt, task);
    }
}

/// `DashPay::set_private_details` (m4 §2.4, §7 item 42).
pub(super) async fn set(
    net: &impl RequestNet,
    session: &Arc<NetworkSession>,
    wallet: &PlatformWallet,
    pair: Pair,
    d: PrivateDetails,
    grant: Option<String>,
) -> Result<PublishState, ContactError> {
    if pair.identity == pair.contact {
        return Err(ContactError::IsSelf);
    }
    let d = normalized(d);
    let bytes = encoded(&d).len();
    if bytes > MAX_PLAINTEXT_LEN {
        return Err(ContactError::DetailsTooLong {
            bytes: u32::try_from(bytes).unwrap_or(u32::MAX),
            max: MAX_PLAINTEXT_LEN as u32,
        });
    }
    let locks = identity_locks(pair.wallet, pair.identity);
    let _write = locks.write.lock().await;
    let mut mine = Mine::of(wallet, &pair).await?;
    let not_contact = || PlatformError::InvalidArgument {
        detail: "not a contact of this identity".into(),
    };
    if mine.details.is_none() {
        return Err(not_contact().into());
    }
    if mine.contacts < 2 {
        save_local(wallet, &pair, &d).await?;
        return Ok(PublishState::DeferredUntilTwoContacts);
    }
    let mut recorded = session.recorded_digest(&pair).await?;
    let Some(grant) = grant else {
        // With nothing recorded, Platform may still hold details (a settle
        // that failed): read them first, when the vault allows it.
        if recorded.is_none()
            && mine.details.as_ref() != Some(&d)
            && let Some(index) = mine.index
            && let Some(crypto) = session.prompt_free_crypto(pair.wallet).await
        {
            let at = (pair.wallet, index, pair.identity);
            match session.settle_locked(wallet, at, &crypto).await {
                Ok(()) => {
                    mine = Mine::of(wallet, &pair).await?;
                    recorded = session.recorded_digest(&pair).await?;
                }
                Err(e) => {
                    tracing::info!(contact = %b58(&pair.contact), "private details not read: {e}")
                }
            }
        }
        let before = mine.details.ok_or_else(not_contact)?;
        // Platform holds details for the contact: the library's next pass
        // would put them back over a local-only edit.
        if recorded.is_some() && d != before {
            return Err(PlatformError::NeedsGrant {
                purpose: BudgetPurpose::Credits,
            }
            .into());
        }
        save_local(wallet, &pair, &d).await?;
        return Ok(publish_state(mine.contacts, &d, recorded));
    };
    let before = mine.details.ok_or_else(not_contact)?;
    if before == d && recorded == Some(digest(&d)) {
        return Ok(PublishState::Published);
    }
    let index = mine.index.ok_or_else(|| PlatformError::InvalidArgument {
        detail: "the wallet only watches this identity; it cannot publish".into(),
    })?;
    if !missing_dashpay_purposes(mine.identity.public_keys().values()).is_empty() {
        return Err(ContactError::Ineligible {
            reason: Eligibility::NoDashPayKeys,
        });
    }
    let (lease, permit, signer, crypto) = prepare(
        net,
        session,
        (index, &mine.identity),
        (pair.wallet, &grant),
        (
            &[FlowKind::PrivateDetails],
            DOCUMENT_10K_CREDITS,
            "set_contact_info",
        ),
        || Ok(()),
    )
    .await?;
    let written = leased(
        &lease,
        net.set_contact_info(wallet, pair.identity, pair.contact, &d, &signer, &crypto),
    )
    .await;
    drop(signer);
    let outcome = permit.finish(written.is_ok());
    match written {
        Ok(ContactInfoPublishOutcome::Published) => {
            session.record_digest(&pair, &d).await?;
            Ok(PublishState::Published)
        }
        // The library counted fewer contacts than this call did; it saved
        // the details.
        Ok(ContactInfoPublishOutcome::DeferredUntilTwoContacts) => {
            Ok(PublishState::DeferredUntilTwoContacts)
        }
        Ok(ContactInfoPublishOutcome::SkippedWatchOnly) => Ok(PublishState::Local),
        Err(e) if outcome == FlowOutcome::Cancelled => {
            // Nothing signed, so nothing went out. The library saved the
            // details first: they go back where Platform holds others (its
            // next pass would put those back), else they stay, `Local`.
            if recorded.is_some() {
                save_local(wallet, &pair, &before).await?;
            }
            Err(e.into())
        }
        Err(e) => {
            // Signed, so it may have landed, or land later. Details the
            // read-back finds are `Published`; otherwise the edit stays,
            // `Local`, and a later read decides (the library's pass puts
            // Platform's details back meanwhile, as it would anyway).
            tracing::info!(contact = %b58(&pair.contact), "contactInfo write failed: {e}");
            let read = read_platform(
                &*session.info_source()?,
                &crypto,
                (session.vault.network(), index, pair.identity),
            )
            .await;
            match read {
                Ok(found) if found.get(&pair.contact) == Some(&d) => {
                    session.record_digest(&pair, &d).await?;
                    Ok(PublishState::Published)
                }
                read => {
                    if let Err(again) = read {
                        tracing::info!("after the failure: {again}");
                    }
                    session.resettle_later(pair.wallet, pair.identity);
                    Err(e.into())
                }
            }
        }
    }
}
