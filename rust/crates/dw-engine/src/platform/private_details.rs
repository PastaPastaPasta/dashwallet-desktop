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

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dash_sdk::platform::{DocumentQuery, FetchMany};
use dashcore::hashes::{Hash, sha256};
use dpp::document::{Document, DocumentV0Getters};
use dpp::identity::Identity;
use dpp::identity::accessors::IdentityGettersV0;
use dpp::platform_value::Value;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dw_appdb::{AppDb, AppDbError};
use platform_wallet::wallet::identity::crypto::contact_info::{
    ContactInfoPrivateData, MAX_PLAINTEXT_LEN, decode_private_data, encode_private_data,
};
use platform_wallet::{
    ContactInfoPublishOutcome, EstablishedContact, PlatformWallet, PlatformWalletError,
};
use tokio::time::Instant;

pub(super) use super::contact_names::BoxedFuture;
use super::contacts::{Eligibility, PrivateDetails, PublishState};
use super::errors::{ContactError, PlatformError};
use super::flows::{BudgetPurpose, DOCUMENT_10K_CREDITS, FlowKind};
use super::identity_writes::identity_locks;
use super::keys_policy::{key_path, missing_dashpay_purposes};
use super::lease::FlowOutcome;
use super::registration_flow::b58;
use super::requests::{Pair, RequestNet, leased, not_found, prepare};
use super::runtime::guard;
use super::signers::VaultContactCrypto;
use super::startup::StartupStatus;
use crate::events::unix_now;
use crate::session::Manager;
use crate::{NetworkSession, PlatformChange, WalletId};

/// `contactInfo` documents per page of a read.
const PAGE: u32 = 100;
/// The most documents one identity's read takes, counted as Platform
/// returns them (parsed or not); more fails the read and refuses a
/// publish. An identity has one per contact it published details for.
const MAX_DOCUMENTS: usize = 1000;
/// How long one identity's settle may take, as a contact's username fetch
/// (`contact_names.rs`): the seed-binding check, the fetch and the opens
/// under one deadline, checked between opens.
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
            let (mut out, mut raw) = (Vec::new(), 0);
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
                // Every entry counts, whether it parses or not.
                raw += len;
                if raw > MAX_DOCUMENTS {
                    return Err(too_many());
                }
                out.extend(
                    page.into_values()
                        .flatten()
                        .filter_map(|d| document(&owner, &d)),
                );
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

/// More `contactInfo` documents than the engine reads.
fn too_many() -> PlatformError {
    PlatformError::NotImplemented {
        call: format!("contactInfo with more than {MAX_DOCUMENTS} documents"),
    }
}

/// What `dp_contact_info` records, in memory for the sync reads (m4 §1:
/// `contact` reads memory only): loaded per wallet in the background on
/// its first read, then written through by every record and forget.
#[derive(Default)]
pub(crate) struct DigestCache(Mutex<BTreeMap<WalletId, Cached>>, AtomicU64);

#[derive(Default)]
struct Cached {
    /// `None` until loaded.
    rows: Option<BTreeMap<(Identifier, Identifier), [u8; 32]>>,
    /// New at every write, so a load that read before it reads again;
    /// drawn from one counter, so a forget does not bring an old one back.
    epoch: u64,
    loading: bool,
}

impl DigestCache {
    /// The digest of `owner`'s details of `contact`; `None` too while the
    /// wallet's digests are not loaded.
    pub(super) fn get(
        &self,
        id: WalletId,
        owner: &Identifier,
        contact: &Identifier,
    ) -> Option<[u8; 32]> {
        let cache = guard(&self.0);
        let rows = cache.get(&id)?.rows.as_ref()?;
        rows.get(&(*owner, *contact)).copied()
    }

    fn next_epoch(&self) -> u64 {
        self.1.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Starts loading wallet `id`: the epoch the load reads at, `None` when
    /// it is loaded or loading.
    fn begin_load(&self, id: WalletId) -> Option<u64> {
        let mut cache = guard(&self.0);
        let c = cache.entry(id).or_insert_with(|| Cached {
            epoch: self.next_epoch(),
            ..Cached::default()
        });
        if c.rows.is_some() || c.loading {
            return None;
        }
        c.loading = true;
        Some(c.epoch)
    }

    /// Lands a load that read at `epoch`, or nothing when the wallet was
    /// forgotten since; else (a write came in between) the epoch to read at
    /// again.
    fn finish_load(
        &self,
        id: WalletId,
        epoch: u64,
        rows: BTreeMap<(Identifier, Identifier), [u8; 32]>,
    ) -> Result<(), u64> {
        let mut cache = guard(&self.0);
        let Some(c) = cache.get_mut(&id) else {
            return Ok(());
        };
        if c.epoch != epoch {
            return Err(c.epoch);
        }
        (c.rows, c.loading) = (Some(rows), false);
        Ok(())
    }

    /// A load that failed: the next read tries again.
    fn abort_load(&self, id: WalletId) {
        if let Some(c) = guard(&self.0).get_mut(&id) {
            c.loading = false;
        }
    }

    /// After `dp_contact_info` changed for (`owner`, `contact`).
    fn write(
        &self,
        id: WalletId,
        owner: Identifier,
        contact: Identifier,
        digest: Option<[u8; 32]>,
    ) {
        let mut cache = guard(&self.0);
        let c = cache.entry(id).or_default();
        c.epoch = self.next_epoch();
        if let Some(rows) = &mut c.rows {
            match digest {
                Some(d) => rows.insert((owner, contact), d),
                None => rows.remove(&(owner, contact)),
            };
        }
    }

    /// Tests: whether anything of wallet `id` is in memory.
    #[cfg(test)]
    pub(super) fn holds(&self, id: &WalletId) -> bool {
        guard(&self.0).contains_key(id)
    }

    /// Nothing of wallet `id` stays in memory (it left the manager, or a
    /// restore of it rolled back): its next read loads again, and a load
    /// of it in flight lands nothing.
    pub(crate) fn forget(&self, id: &WalletId) {
        guard(&self.0).remove(id);
    }
}

/// Private-details writes of a wallet (its library details, its
/// `dp_contact_info` rows and their digests in memory) against the wallet
/// leaving the manager: a write lands only while the wallet it read is
/// still the manager's, and the wallet leaves only once the writes in
/// flight have landed. So a settle or a publish that outlives a removal
/// writes nothing of the removed wallet.
#[derive(Default)]
pub(crate) struct WriteFences(Mutex<BTreeMap<WalletId, Arc<tokio::sync::RwLock<()>>>>);

/// Held over private-details writes: the wallet was the manager's when it
/// was taken, and stays until it is dropped. Shared with the blocking job
/// of each app.sqlite write, which keeps it until the job ends.
pub(super) struct WriteFence(WalletId, Arc<tokio::sync::OwnedRwLockReadGuard<()>>);

impl WriteFences {
    fn fence(&self, id: WalletId) -> Arc<tokio::sync::RwLock<()>> {
        Arc::clone(guard(&self.0).entry(id).or_default())
    }

    /// Tests: wallet `id`'s fence.
    #[cfg(test)]
    pub(super) fn of(&self, id: WalletId) -> Arc<tokio::sync::RwLock<()>> {
        self.fence(id)
    }

    /// Held while wallet `id` leaves the manager.
    pub(super) async fn removal(&self, id: WalletId) -> tokio::sync::OwnedRwLockWriteGuard<()> {
        self.fence(id).write_owned().await
    }

    /// The fence for writes of `wallet`, if it is still `manager`'s
    /// wallet `id`.
    async fn enter(
        &self,
        manager: &Manager,
        id: WalletId,
        wallet: &PlatformWallet,
    ) -> Option<WriteFence> {
        let fence = self.fence(id).read_owned().await;
        let live = manager.get_wallet(&id.0).await?;
        std::ptr::eq(&*live, wallet).then(|| WriteFence(id, Arc::new(fence)))
    }
}

/// Tests: what `contactInfo` reads see.
#[cfg(test)]
#[derive(Default)]
pub(super) struct ReadHooks {
    /// The document source is gone (`info_source` fails).
    pub(super) gone: bool,
    /// Replaces [`READ_TIMEOUT`].
    pub(super) budget: Option<Duration>,
    /// Each open waits this long first, as a slow vault would.
    pub(super) open_delay: Option<Duration>,
    /// Opens begun.
    pub(super) opens: usize,
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

/// `pair` as `dp_contact_info` keys it: wallet, identity, contact.
fn row_keys(pair: &Pair) -> (String, String, String) {
    (
        pair.wallet.to_string(),
        b58(&pair.identity),
        b58(&pair.contact),
    )
}

/// Saves `d` as `pair.identity`'s details of `pair.contact` in the library
/// (its changeset tap signals `Contacts`), as its own DashPay pass applies
/// a `contactInfo`. A contact no longer established is left alone.
async fn save_local(
    _fence: &WriteFence,
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
        {
            if guard(&self.platform.contacts.read_hooks).gone {
                return Err(PlatformError::Unavailable);
            }
            if let Some(source) = guard(&self.platform.contacts.info_source).clone() {
                return Ok(source);
            }
        }
        let manager = self.manager()?;
        Ok(Arc::new(SdkInfos(manager.sdk_arc())))
    }

    /// When a settle or a read that starts now must be done.
    fn read_deadline(&self) -> Instant {
        #[cfg(test)]
        if let Some(budget) = guard(&self.platform.contacts.read_hooks).budget {
            return Instant::now() + budget;
        }
        Instant::now() + READ_TIMEOUT
    }

    /// `owner`'s documents from `source` by `deadline`, more than
    /// [`MAX_DOCUMENTS`] refused.
    async fn documents(
        source: &dyn InfoSource,
        owner: Identifier,
        deadline: Instant,
    ) -> Result<Vec<InfoDocument>, PlatformError> {
        let documents = tokio::time::timeout_at(deadline, source.documents(owner))
            .await
            .map_err(|_| PlatformError::Timeout)??;
        if documents.len() > MAX_DOCUMENTS {
            return Err(too_many());
        }
        Ok(documents)
    }

    /// Opens `owner`'s documents (identity `index`) with `crypto`, by
    /// `deadline`: what Platform holds for each contact, the latest update
    /// winning. The deadline and the session's close are checked before each
    /// open, so a read holding the identity's write lock ends at either. A
    /// signer that cannot open them (a locked vault) fails the read; a
    /// document that does not open with the wallet's keys (another client's,
    /// a corrupt one), or whose plaintext does not decode, is skipped, as the
    /// library skips it. Documents of which none opens fail the read too.
    async fn read_platform(
        &self,
        source: &dyn InfoSource,
        crypto: &VaultContactCrypto,
        (index, owner): (u32, Identifier),
        deadline: Instant,
    ) -> Result<BTreeMap<Identifier, PrivateDetails>, PlatformError> {
        let network = self.vault.network();
        let documents = Self::documents(source, owner, deadline).await?;
        let (total, mut opened_any) = (documents.len(), false);
        let mut found = BTreeMap::new();
        for doc in documents {
            if !self.is_open() {
                return Err(PlatformError::NetworkNotOpen);
            }
            if Instant::now() >= deadline {
                return Err(PlatformError::Timeout);
            }
            #[cfg(test)]
            {
                let delay = {
                    let mut hooks = guard(&self.platform.contacts.read_hooks);
                    hooks.opens += 1;
                    hooks.open_delay
                };
                if let Some(delay) = delay {
                    tokio::time::sleep(delay).await;
                }
            }
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
            // A point where a close's abort lands.
            tokio::task::yield_now().await;
            let Some(opened) = opened else {
                tracing::warn!(owner = %owner, "a contactInfo that does not open with the wallet's keys");
                continue;
            };
            opened_any = true;
            match decode_private_data(&opened.private_data) {
                Ok(data) => {
                    found.insert(Identifier::from(opened.contact_id), data.into());
                }
                Err(e) => {
                    tracing::warn!(owner = %owner, error = %e, "a contactInfo that does not decode")
                }
            }
        }
        // The last open too.
        if Instant::now() >= deadline {
            return Err(PlatformError::Timeout);
        }
        if total > 0 && !opened_any {
            return Err(PlatformError::Internal {
                detail: format!(
                    "none of {total} contactInfo documents opens with the wallet's keys"
                ),
            });
        }
        Ok(found)
    }

    /// A settle requires a verified seed binding: `crypto` must derive the
    /// wallet's own account-0 xpub (platform-wallet `verify_seed_binds`),
    /// checked here for every settle. Without it, a read that finds nothing
    /// could not tell "Platform holds nothing" from keys of another seed.
    /// A proven mismatch is the wallet's `SeedBindingUnverified`, as the
    /// unlock drain reports it.
    async fn verify_binding(
        &self,
        wallet: &PlatformWallet,
        id: WalletId,
        crypto: &VaultContactCrypto,
        deadline: Instant,
    ) -> Result<(), PlatformError> {
        let checked = tokio::time::timeout_at(deadline, wallet.verify_seed_binds(crypto))
            .await
            .map_err(|_| PlatformError::Timeout)?;
        match checked {
            Ok(()) => Ok(()),
            Err(e @ PlatformWalletError::SeedMismatch { .. }) => {
                let status = StartupStatus::SeedBindingUnverified;
                // Noticed once, by whichever path finds it first.
                if self.platform.set_status(id, status) != Some(status) {
                    tracing::error!(wallet_id = %id, error = %e, "private details not settled: the vault's seed does not bind this wallet");
                    self.notice_incomplete(id, status);
                }
                Err(PlatformError::SeedMismatch)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Loads wallet `id`'s digests into memory in the background, once
    /// ([`DigestCache`]); signals `Contacts` for the identities that have
    /// any when they land.
    pub(super) fn load_digests(&self, id: WalletId) {
        let cache = Arc::clone(&self.platform.contacts.digests);
        let Some(mut epoch) = cache.begin_load(id) else {
            return;
        };
        let Ok(live) = self.live() else {
            cache.abort_load(id);
            return;
        };
        let (db, manager, hub) = (live.appdb, live.manager, Arc::clone(&self.hub));
        let fences = Arc::clone(&self.platform.contacts.fences);
        #[cfg(test)]
        let pause = guard(&self.platform.contacts.load_pause).take();
        self.platform.contacts.spawn(&self.rt, async move {
            let Some(loaded) = manager.get_wallet(&id.0).await else {
                cache.abort_load(id);
                return;
            };
            #[cfg(test)]
            let mut pause = pause;
            loop {
                let (db, wallet) = (Arc::clone(&db), id.to_string());
                let read =
                    tokio::task::spawn_blocking(move || db.contact_info_digests(&wallet)).await;
                let Ok(Ok(read)) = read else {
                    tracing::warn!(wallet_id = %id, "contactInfo digests not loaded");
                    cache.abort_load(id);
                    return;
                };
                let id58 = |t: &str| Identifier::from_string(t, Encoding::Base58).ok();
                let rows: BTreeMap<_, _> = (read.iter())
                    .filter_map(|(i, c, d)| Some(((id58(i)?, id58(c)?), *d)))
                    .collect();
                let owners: BTreeSet<Identifier> = rows.keys().map(|(o, _)| *o).collect();
                #[cfg(test)]
                if let Some(pause) = pause.take() {
                    pause
                        .reached
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    pause.release.notified().await;
                }
                let Some(_fence) = fences.enter(&manager, id, &loaded).await else {
                    cache.abort_load(id);
                    return;
                };
                match cache.finish_load(id, epoch, rows) {
                    Ok(()) => {
                        let changes =
                            (owners.iter()).map(|o| PlatformChange::Contacts { identity: b58(o) });
                        hub.pump.mark_platform(id, changes);
                        return;
                    }
                    Err(now) => epoch = now,
                }
            }
        });
    }

    /// The digest `dp_contact_info` holds for `pair`.
    pub(super) async fn recorded_digest(
        &self,
        pair: &Pair,
    ) -> Result<Option<[u8; 32]>, PlatformError> {
        let (w, i, c) = row_keys(pair);
        Ok(self
            .appdb_op(move |db| db.contact_info_digest(&w, &i, &c))
            .await?)
    }

    /// `f` on app.sqlite, the private-details write `what` under `fence`:
    /// its blocking job holds the fence until it ends, even when the task
    /// awaiting it is cancelled, so the wallet leaves after the write.
    async fn fenced_op<T: Send + 'static>(
        &self,
        fence: &WriteFence,
        what: &'static str,
        f: impl FnOnce(&AppDb) -> Result<T, AppDbError> + Send + 'static,
    ) -> Result<T, PlatformError> {
        let (wallet_id, held) = (fence.0, Arc::clone(&fence.1));
        #[cfg(test)]
        let pause = {
            let mut slot = guard(&self.platform.contacts.job_pause);
            slot.take_if(|(at, _)| *at == what).map(|(_, pause)| pause)
        };
        let written = self
            .appdb_op(move |db| {
                let _held = held;
                #[cfg(test)]
                if let Some(pause) = pause {
                    pause.hold();
                }
                f(db)
            })
            .await;
        written.map_err(|e| {
            tracing::warn!(%wallet_id, write = what, error = %e, "private details not written");
            e.into()
        })
    }

    /// Records that Platform holds `d` for `pair`, in app.sqlite and in
    /// memory, by one job. A change signals `Contacts`: the details'
    /// `publish_state` moved.
    async fn record_digest(
        &self,
        fence: &WriteFence,
        pair: &Pair,
        d: &PrivateDetails,
    ) -> Result<(), PlatformError> {
        let (digest, cache, at) = (digest(d), self.digests(), *pair);
        let (w, i, c) = row_keys(pair);
        let changed = (self.fenced_op(fence, "record", move |db| {
            let changed = db.set_contact_info_digest(&w, &i, &c, &digest, unix_now())?;
            cache.write(at.wallet, at.identity, at.contact, Some(digest));
            Ok(changed)
        }))
        .await?;
        self.digest_changed(pair, changed);
        Ok(())
    }

    /// Forgets what was recorded for `pair`: Platform holds nothing for it.
    async fn forget_digest(&self, fence: &WriteFence, pair: &Pair) -> Result<(), PlatformError> {
        let (cache, at) = (self.digests(), *pair);
        let (w, i, c) = row_keys(pair);
        let changed = (self.fenced_op(fence, "forget", move |db| {
            let changed = db.forget_contact_info_digest(&w, &i, &c)?;
            cache.write(at.wallet, at.identity, at.contact, None);
            Ok(changed)
        }))
        .await?;
        self.digest_changed(pair, changed);
        Ok(())
    }

    fn digests(&self) -> Arc<DigestCache> {
        Arc::clone(&self.platform.contacts.digests)
    }

    /// The fence for private-details writes of `wallet` ([`WriteFences`]);
    /// `WalletNotFound` once it is no longer the manager's wallet `id`.
    async fn write_fence(
        &self,
        id: WalletId,
        wallet: &PlatformWallet,
    ) -> Result<WriteFence, PlatformError> {
        let manager = self.manager()?;
        let fences = &self.platform.contacts.fences;
        (fences.enter(&manager, id, wallet).await).ok_or(PlatformError::WalletNotFound)
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
    /// nothing for loses its record, and so does a contact no longer
    /// established. It needs a verified seed binding first
    /// ([`Self::verify_binding`]); a failed check or read changes nothing.
    /// The lock is held across the read (all of it within one
    /// [`READ_TIMEOUT`]), so a write of the identity cannot land between the
    /// read and what it applies.
    pub(super) async fn settle_identity(
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
        let deadline = self.read_deadline();
        self.verify_binding(wallet, id, crypto, deadline).await?;
        let source = self.info_source()?;
        let found = (self.read_platform(&*source, crypto, (index, owner), deadline)).await?;
        let established: Vec<(Identifier, PrivateDetails)> = {
            let state = wallet.state().await;
            let Some(m) = state.identity_manager.managed_identity(&owner) else {
                return Ok(());
            };
            (m.dashpay().established_contacts().iter())
                .map(|(contact, c)| (*contact, c.into()))
                .collect()
        };
        #[cfg(test)]
        crate::coins::pause_at(&self.platform.contacts.settle_pause).await;
        // The apply phase starts within the deadline: local writes only, one
        // set per established contact, so it then runs to its end.
        if Instant::now() >= deadline {
            return Err(PlatformError::Timeout);
        }
        let fence = self.write_fence(id, wallet).await?;
        #[cfg(test)]
        crate::coins::pause_at(&self.platform.contacts.apply_pause).await;
        let keep: Vec<String> = established.iter().map(|(c, _)| b58(c)).collect();
        for (contact, shown) in established {
            let pair = Pair {
                wallet: id,
                identity: owner,
                contact,
            };
            match found.get(&contact) {
                Some(held) => {
                    if *held != shown {
                        save_local(&fence, wallet, &pair, held).await?;
                    }
                    self.record_digest(&fence, &pair, held).await?;
                }
                None => self.forget_digest(&fence, &pair).await?,
            }
        }
        // Records of contacts no longer established go too.
        let (w, i, cache) = (id.to_string(), b58(&owner), self.digests());
        let gone = (self.fenced_op(&fence, "gone", move |db| {
            let gone: Vec<Identifier> = (db.forget_contact_infos_except(&w, &i, &keep)?)
                .iter()
                .filter_map(|c| Identifier::from_string(c, Encoding::Base58).ok())
                .collect();
            for contact in &gone {
                cache.write(id, owner, *contact, None);
            }
            Ok(gone)
        }))
        .await?;
        for contact in gone {
            let pair = Pair {
                wallet: id,
                identity: owner,
                contact,
            };
            self.digest_changed(&pair, true);
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

    /// [`Self::settle_contact_infos`] for wallet `id` in the background, so
    /// a bring-up or an unlock drain does not wait for Platform (a read may
    /// take [`READ_TIMEOUT`] per identity). Session close cancels it.
    pub(super) fn settle_later(self: &Arc<Self>, id: WalletId, crypto: VaultContactCrypto) {
        let weak = Arc::downgrade(self);
        let task = async move {
            let Some(this) = weak.upgrade() else { return };
            let Ok(manager) = this.manager() else { return };
            if let Some(wallet) = manager.get_wallet(&id.0).await {
                this.settle_contact_infos(&wallet, id, &crypto, None).await;
            }
        };
        self.platform.contacts.spawn(&self.rt, task);
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
        let fence = session.write_fence(pair.wallet, wallet).await?;
        save_local(&fence, wallet, &pair, &d).await?;
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
        let fence = session.write_fence(pair.wallet, wallet).await?;
        save_local(&fence, wallet, &pair, &d).await?;
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
    // The library's publish reads every document the identity holds, with
    // no bound of its own: refused before it when there are more than the
    // engine reads (m4 §7 item 42).
    let source = session.info_source()?;
    let deadline = session.read_deadline();
    NetworkSession::documents(&*source, pair.identity, deadline).await?;
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
    #[cfg(test)]
    crate::coins::pause_at(&session.platform.contacts.publish_pause).await;
    match written {
        Ok(ContactInfoPublishOutcome::Published) => {
            let fence = session.write_fence(pair.wallet, wallet).await?;
            session.record_digest(&fence, &pair, &d).await?;
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
                let fence = session.write_fence(pair.wallet, wallet).await?;
                save_local(&fence, wallet, &pair, &before).await?;
            }
            Err(e.into())
        }
        Err(e) => {
            // Signed, so it may have landed, or land later. Details the
            // read-back finds are `Published`; otherwise the edit stays,
            // `Local`, and a later read decides (the library's pass puts
            // Platform's details back meanwhile, as it would anyway).
            tracing::info!(contact = %b58(&pair.contact), "contactInfo write failed: {e}");
            let read = match session.info_source() {
                Ok(source) => {
                    let (at, deadline) = ((index, pair.identity), session.read_deadline());
                    (session.read_platform(&*source, &crypto, at, deadline)).await
                }
                Err(e) => Err(e),
            };
            match read {
                Ok(found) if found.get(&pair.contact) == Some(&d) => {
                    let fence = session.write_fence(pair.wallet, wallet).await?;
                    session.record_digest(&fence, &pair, &d).await?;
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
