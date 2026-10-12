//! Other users (DP2-03): DPNS prefix search and exact lookup, each hit with
//! its profile and its relation to the main identity; an own identity's
//! `dashpay://user` link; and scanned codes verified against Platform.
//!
//! - Search and lookup read the domain documents and profiles the re-fetch
//!   reads (`reverify.rs`): the names in one trust scope, then the hits'
//!   profiles in one query in another. A hit is `unverified` when the
//!   fallback is latched and neither the fetch that showed a part verified
//!   against SPV-held quorum keys nor a record covers the content shown
//!   (m4 §5). A search records nothing: the re-fetch verifies the data a
//!   read model shows.
//! - The user link is iOS's (`DashPayUserLink.swift`): its main name, and a
//!   scanned link verifies as iOS's `handleScannedCode` does, by an exact
//!   DPNS lookup that must return the scanned identity.
//! - A DIP-15 code (`dash:?du=&dapk=`) carries an auto-accept key. It stays
//!   here as a scan for DP2-02's send, which claims it, signs the proof with
//!   it and spends it once sent ([`send_seam`]); the host gets only the scan
//!   id. Neither app makes such codes yet (DEC-15).
//! - Neither code is signed. A scan shows that its username belongs to its
//!   identity now, not who made the QR, and a code's key is only claimed to
//!   be that identity's: it derives on a hardened path of the owner's seed
//!   (`m/9'/coin'/16'/expiry'`) that nothing published reveals, so only the
//!   recipient can check it, when the request arrives. Auto-accept stays
//!   unconfirmed until the recipient accepts.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use dash_sdk::drive::config::DEFAULT_QUERY_LIMIT;
use dash_sdk::platform::dpns_usernames::is_valid_username;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dpp::util::strings::convert_to_homograph_safe_chars;
use dw_uri::deeplink::{DashPayContactCode, DashPayUserLink, dpns_label};

use super::contacts::{ScannedCode, ScannedContact};
use super::contacts_model::identifier;
use super::errors::{ContactError, IdentityError, PlatformError};
use super::names::UserHit;
use super::reverify::{Held, Refetcher, SdkRefetcher, ShownProfile};
use super::runtime::guard;
use super::trust::{self, BoxFuture};
use crate::{NetworkSession, WalletId};
use send_seam::Scan;

/// Scans a session keeps that no send holds; the oldest goes first.
const MAX_SCANS: usize = 16;

/// Platform's answers for user lookups: the SDK, or a test's script.
pub(super) trait UserNet: Send + Sync {
    /// The DPNS names whose normalized label starts with `prefix` (already
    /// normalized), in Platform's order (by normalized label), at most
    /// `limit`: each label as spelled and its `records.identity`.
    fn search(
        &self,
        prefix: String,
        limit: u32,
    ) -> BoxFuture<Result<Vec<(String, Identifier)>, PlatformError>>;
    /// The domain of the normalized `label`: its label as spelled and its
    /// `records.identity`.
    fn domain(
        &self,
        label: String,
    ) -> BoxFuture<Result<Option<(String, Identifier)>, PlatformError>>;
    /// The profiles of `ids` (distinct, at most 100) as the read models
    /// show them, in one query: identities without one are absent.
    fn profiles(
        &self,
        ids: Vec<Identifier>,
    ) -> BoxFuture<Result<Vec<(Identifier, ShownProfile)>, PlatformError>>;
}

/// The SDK's queries: the re-fetch's domain and profile reads, and its
/// prefix search.
struct SdkUsers(SdkRefetcher);

impl UserNet for SdkUsers {
    fn search(
        &self,
        prefix: String,
        limit: u32,
    ) -> BoxFuture<Result<Vec<(String, Identifier)>, PlatformError>> {
        let sdk = Arc::clone(&self.0.0);
        Box::pin(async move {
            let names = sdk.search_dpns_names(&prefix, Some(limit)).await?;
            Ok((names.into_iter())
                .filter_map(|n| Some((n.label, n.records_identity_id?)))
                .collect())
        })
    }

    fn domain(
        &self,
        label: String,
    ) -> BoxFuture<Result<Option<(String, Identifier)>, PlatformError>> {
        self.0.dpns_domain(label)
    }

    fn profiles(
        &self,
        ids: Vec<Identifier>,
    ) -> BoxFuture<Result<Vec<(Identifier, ShownProfile)>, PlatformError>> {
        self.0.profiles(ids)
    }
}

/// The session's user lookups: the scans waiting for a send, and a test's
/// network.
#[derive(Default)]
pub(crate) struct UsersRuntime {
    pub(crate) scans: Arc<Scans>,
    #[cfg(test)]
    pub(super) net: Mutex<Option<Arc<dyn UserNet>>>,
}

/// The session's scans, oldest first, under one lock with each wallet's
/// load generation: [`Scans::forget`] (a removal or unload) starts a new
/// one, and a scan begun in an older one is refused, so a lookup that was
/// paused across an unload and reload cannot leave a key behind. Closing
/// the session drops them all for good.
#[derive(Default)]
pub(crate) struct Scans(Mutex<ScanStore>);

#[derive(Default)]
struct ScanStore {
    scans: VecDeque<(String, Scan)>,
    generations: HashMap<WalletId, u64>,
    closed: bool,
}

impl ScanStore {
    fn generation(&self, wallet: WalletId) -> u64 {
        self.generations.get(&wallet).copied().unwrap_or(0)
    }

    /// Where `wallet`'s scan `id` is.
    fn position(&self, wallet: WalletId, id: &str) -> Option<usize> {
        (self.scans.iter()).position(|(i, s)| i == id && s.wallet == wallet)
    }
}

impl Scans {
    /// The wallet's load generation, to pass to [`Self::insert`]. Read it
    /// before checking that the wallet is loaded.
    fn generation(&self, wallet: WalletId) -> u64 {
        guard(&self.0).generation(wallet)
    }

    /// Keeps `scan`, begun in the wallet's load `generation`, dropping the
    /// expired ones (their keys are of no use) and, at [`MAX_SCANS`], the
    /// oldest; a scan a send holds stays. A newer generation:
    /// `wallet_not_found`; a closed session: `network_not_open`.
    fn insert(&self, scan: Scan, generation: u64, now: u64) -> Result<String, PlatformError> {
        let mut store = guard(&self.0);
        if store.closed {
            return Err(PlatformError::NetworkNotOpen);
        }
        if store.generation(scan.wallet) != generation {
            return Err(PlatformError::WalletNotFound);
        }
        let id = format!("scan-{}", hex::encode(rand::random::<[u8; 12]>()));
        store.scans.retain(|(_, s)| s.claimed || !s.expired(now));
        let free = (store.scans.iter()).filter(|(_, s)| !s.claimed).count();
        if free >= MAX_SCANS
            && let Some(oldest) = (store.scans.iter()).position(|(_, s)| !s.claimed)
        {
            store.scans.remove(oldest);
        }
        store.scans.push_back((id.clone(), scan));
        Ok(id)
    }

    /// Drops the wallet's scans and starts its next load generation
    /// (removal, unload).
    pub(crate) fn forget(&self, wallet: &WalletId) {
        let mut store = guard(&self.0);
        store.scans.retain(|(_, s)| s.wallet != *wallet);
        *store.generations.entry(*wallet).or_default() += 1;
    }

    /// Drops every scan; none is kept again (the session closes).
    pub(crate) fn close(&self) {
        let mut store = guard(&self.0);
        store.scans.clear();
        store.closed = true;
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        guard(&self.0).scans.len()
    }
}

fn invalid(detail: &str) -> PlatformError {
    PlatformError::InvalidArgument {
        detail: detail.into(),
    }
}

impl NetworkSession {
    fn user_net(&self) -> Result<Arc<dyn UserNet>, PlatformError> {
        #[cfg(test)]
        if let Some(net) = guard(&self.platform.users.net).clone() {
            return Ok(net);
        }
        let manager = self.manager()?;
        Ok(Arc::new(SdkUsers(SdkRefetcher(manager.sdk_arc()))))
    }

    /// `DashPay::search_users`. Only the normalized prefix goes to DAPI; a
    /// prefix no label can start with asks nothing.
    pub(super) async fn search_users_of(
        &self,
        wallet: WalletId,
        prefix: String,
        limit: u32,
    ) -> Result<Vec<UserHit>, PlatformError> {
        let _op = self.enter().await?;
        self.require_wallet(&wallet)?;
        let prefix = convert_to_homograph_safe_chars(prefix.trim());
        if prefix.is_empty() || limit == 0 {
            return Err(invalid("search needs a prefix and a limit above 0"));
        }
        if !(prefix.bytes()).all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
            return Ok(Vec::new());
        }
        let net = self.user_net()?;
        let limit = limit.min(u32::from(DEFAULT_QUERY_LIMIT));
        let (found, scope) = trust::scoped(net.search(prefix, limit)).await;
        self.hits(wallet, &*net, found?, scope.verified()).await
    }

    /// `DashPay::resolve_user`: `alice` or `alice.dash`, any case.
    pub(super) async fn resolve_user_of(
        &self,
        wallet: WalletId,
        username: String,
    ) -> Result<Option<UserHit>, PlatformError> {
        let _op = self.enter().await?;
        self.require_wallet(&wallet)?;
        let label = dpns_label(username.trim());
        Ok(self.lookup(wallet, label).await?.map(|(_, hit)| hit))
    }

    /// The user who owns `label` now (no `.dash`: each caller drops it once,
    /// as its form says), with the hit shown for them. A label DPNS could
    /// not hold asks nothing.
    async fn lookup(
        &self,
        wallet: WalletId,
        label: &str,
    ) -> Result<Option<(Identifier, UserHit)>, PlatformError> {
        if !is_valid_username(label) {
            return Ok(None);
        }
        let net = self.user_net()?;
        let label = convert_to_homograph_safe_chars(label);
        let (found, scope) = trust::scoped(net.domain(label)).await;
        let Some(found) = found? else {
            return Ok(None);
        };
        let owner = found.1;
        let hits = self
            .hits(wallet, &*net, vec![found], scope.verified())
            .await?;
        Ok(hits.into_iter().next().map(|hit| (owner, hit)))
    }

    /// The hits for `found` (labels as spelled, with their identities),
    /// whose labels' fetch verified if `labels_verified`: each with its
    /// profile, all fetched in one query and scope (a failed fetch shows
    /// none), and its relation.
    async fn hits(
        &self,
        wallet: WalletId,
        net: &dyn UserNet,
        found: Vec<(String, Identifier)>,
        labels_verified: bool,
    ) -> Result<Vec<UserHit>, PlatformError> {
        if found.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<Identifier> = found.iter().map(|(_, id)| *id).collect();
        let distinct = ids.iter().copied().collect::<BTreeSet<_>>();
        let (profiles, scope) = trust::scoped(net.profiles(distinct.into_iter().collect())).await;
        let profiles: HashMap<Identifier, ShownProfile> = (profiles)
            .inspect_err(|e| tracing::debug!(error = %e, "could not fetch the users' profiles"))
            .unwrap_or_default()
            .into_iter()
            .collect();
        let profiles_verified = scope.verified();
        let relations = self.relations_to_main(wallet, &ids)?;
        let provenance = self.live()?.provenance;
        let unverified =
            |held: Held, verified: bool| !verified && provenance.is_unverified(&held.entity());
        Ok((found.into_iter().zip(relations))
            .map(|((spelling, id), relation)| {
                let profile = profiles.get(&id);
                let label_unverified = unverified(Held::label(&spelling, id), labels_verified);
                let profile_unverified = profile.is_some_and(|shown| {
                    let shown = shown.clone();
                    unverified(Held::Profile { id, shown }, profiles_verified)
                });
                UserHit {
                    identity: id.to_string(Encoding::Base58),
                    username: spelling,
                    display_name: profile.and_then(|p| p.display_name.clone()),
                    avatar_url: profile.and_then(|p| p.avatar_url.clone()),
                    relation,
                    unverified: label_unverified || profile_unverified,
                }
            })
            .collect())
    }

    /// `DashPay::my_user_link`: the identity's main name as iOS's link. No
    /// name: `contact.no_username`; names not known while a sync pass holds
    /// the identities: `platform.unavailable`.
    pub(super) fn my_user_link_of(
        &self,
        wallet: WalletId,
        identity: &str,
    ) -> Result<String, ContactError> {
        let summaries = self
            .identity_summaries(wallet)
            .map_err(PlatformError::from)?;
        let id = identifier(identity)?;
        let Some(own) = (summaries.iter())
            .find(|s| Identifier::from_string(&s.identity, Encoding::Base58).ok() == Some(id))
        else {
            // Before the first snapshot, a pass holding the identities
            // hides them all.
            let manager = self.manager().map_err(PlatformError::from)?;
            let held = manager.wallet_manager_arc().try_read().is_err();
            return Err(if held {
                PlatformError::Unavailable
            } else {
                IdentityError::NotFound.into()
            }
            .into());
        };
        if own.names_updating {
            return Err(PlatformError::Unavailable.into());
        }
        let name = own.main_name.as_deref().ok_or(ContactError::NoUsername)?;
        Ok(DashPayUserLink {
            identity_id: id.to_buffer(),
            username: name.to_owned(),
        }
        .to_uri())
    }

    /// `DashPay::verify_scanned`: a user link must name the identity that
    /// owns its username now; a DIP-15 code must not have expired, and its
    /// username must exist. Anything else is `invalid_argument`, which never
    /// quotes the text.
    pub(super) async fn verify_scanned_of(
        &self,
        wallet: WalletId,
        text: &str,
    ) -> Result<ScannedContact, ContactError> {
        let _op = self.enter().await.map_err(PlatformError::from)?;
        let scans = &self.platform.users.scans;
        // Before the wallet check: an unload after it starts a newer one.
        let generation = scans.generation(wallet);
        self.require_wallet(&wallet).map_err(PlatformError::from)?;
        if let Some(link) = DashPayUserLink::parse(text) {
            let claimed = Identifier::from(link.identity_id);
            return match self.lookup(wallet, &link.username).await? {
                Some((owner, hit)) if owner == claimed => Ok(scanned(hit, ScannedCode::UserLink)),
                _ => Err(ContactError::ScanUnverified),
            };
        }
        let code = (DashPayContactCode::parse(text))
            .ok_or_else(|| invalid("not a DashPay contact code"))?;
        let now = crate::events::unix_now();
        let expires_at = u64::from(code.expiry);
        if expires_at <= now {
            return Err(ContactError::ScanExpired);
        }
        let (recipient, hit) =
            (self.lookup(wallet, &code.username).await?).ok_or(ContactError::ScanUnverified)?;
        let scan = Scan {
            wallet,
            recipient,
            key: code.key,
            expiry: code.expiry,
            recipient_unverified: hit.unverified,
            claimed: false,
        };
        let scan = scans.insert(scan, generation, now)?;
        Ok(scanned(
            hit,
            ScannedCode::AutoAcceptClaim { scan, expires_at },
        ))
    }
}

fn scanned(hit: UserHit, code: ScannedCode) -> ScannedContact {
    ScannedContact {
        identity: hit.identity,
        username: Some(hit.username),
        display_name: hit.display_name,
        code,
        relation: hit.relation,
        unverified: hit.unverified,
    }
}

/// The seam DP2-02's `send_request` sends through: the request's sides and,
/// from a scan, a claim on its auto-accept key.
///
/// The contract DP2-02 spends through:
/// - [`NetworkSession::send_request_intent`] claims the scan atomically: one
///   send at a time holds it (another gets `contact.request_pending`), and
///   the claim lives as long as the intent; nothing but the wallet's
///   removal or unload, or the session's close, takes a claimed scan away.
/// - The send does every check that may refuse it (eligibility, grant,
///   vault) first, then gives the claim up for [`ScanClaim::proof_source`]
///   and signs with it through `send_contact_request_with_external_signer`.
///   The call consumes the claim, so one claim makes one proof. platform-wallet's
///   `send_contact_request_from_qr` would parse the code and resolve its
///   name again at send time; the recipient is the identity
///   `verify_scanned` showed, and the library signs once it knows the
///   send's `accountReference`.
/// - Fail closed: a claim dropped before `proof_source` (a send refused)
///   releases the scan for a retry. With the source, `proof_source` returns
///   a [`ScanSpend`] that keeps the scan held: the request may have been
///   broadcast, so dropping it spends the scan whatever happened next (sent,
///   failed, cancelled, panicked). The send keeps it until it is done.
/// - The outcome is `Pending` like any other request: the auto-accept is
///   only claimed (module doc) until the recipient accepts.
pub(crate) mod send_seam {
    use std::sync::Arc;

    use dashcore::secp256k1::SecretKey;
    use dpp::prelude::Identifier;
    use platform_wallet::AutoAcceptProofSource;
    use zeroize::Zeroizing;

    use super::super::contacts_model::identifier;
    use super::{Scans, invalid};
    use crate::platform::errors::ContactError;
    use crate::platform::runtime::guard;
    use crate::{NetworkSession, WalletId};

    /// A verified `dapk` code: whom it lets this wallet's request reach,
    /// auto-accepted as its maker claims, and the key that signs the proof.
    /// The key's only copy in the engine.
    pub(crate) struct Scan {
        pub(super) wallet: WalletId,
        pub(super) recipient: Identifier,
        pub(super) key: Box<Zeroizing<[u8; 32]>>,
        /// When the key stops being honoured: UNIX seconds.
        pub(super) expiry: u32,
        /// The recipient's shown name or profile is unverified (m4 §5).
        pub(super) recipient_unverified: bool,
        /// A send holds it ([`ScanClaim`]).
        pub(super) claimed: bool,
    }

    impl Scan {
        pub(super) fn expired(&self, now: u64) -> bool {
            u64::from(self.expiry) <= now
        }
    }

    /// One contact request to send.
    pub(crate) struct SendRequestIntent {
        pub(crate) sender: Identifier,
        pub(crate) recipient: Identifier,
        /// From a `dapk` scan: the claim on its key, held until the intent
        /// goes.
        pub(crate) auto_accept: Option<ScanClaim>,
    }

    /// One send's hold on a scan (module doc). It holds no key: the proof's
    /// key is read from the scan when the send asks for it.
    pub(crate) struct ScanClaim {
        scans: Arc<Scans>,
        wallet: WalletId,
        id: String,
        /// When the key stops being honoured: UNIX seconds.
        pub(crate) expiry: u32,
        /// The recipient's shown name or profile is unverified (m4 §5): the
        /// send confirms before it relies on Platform's word alone.
        pub(crate) recipient_unverified: bool,
        /// The key went out: the scan is spent when the claim goes.
        used: bool,
    }

    /// A scan whose key went out ([`ScanClaim::proof_source`]): held until
    /// this goes, then spent (module doc).
    #[must_use = "dropping it spends the scan; keep it until the send is done"]
    pub(crate) struct ScanSpend {
        _claim: ScanClaim,
    }

    impl ScanClaim {
        /// The proof source for the send, made once, right before it, and the
        /// guard that spends the scan; the claim goes into the guard. The
        /// bytes `SecretKey::from_secret_bytes` takes by value and the
        /// `SecretKey` handed to platform-wallet are copies this engine
        /// cannot erase: secp256k1 keys are `Copy`, and the library drops
        /// its own unerased once the proof is signed. A scan forgotten
        /// meanwhile (the wallet unloaded, the session closed) or expired by
        /// `now` (the recipient would discard the proof): `contact.scan_expired`.
        /// An error drops the claim, which releases what is left.
        pub(crate) fn proof_source(
            mut self,
            now: u64,
        ) -> Result<(AutoAcceptProofSource, ScanSpend), ContactError> {
            let secret_key = {
                let mut store = guard(&self.scans.0);
                let at =
                    (store.position(self.wallet, &self.id)).ok_or(ContactError::ScanExpired)?;
                if store.scans[at].1.expired(now) {
                    store.scans.remove(at);
                    return Err(ContactError::ScanExpired);
                }
                SecretKey::from_secret_bytes(**store.scans[at].1.key)
                    .expect("a valid scalar: DashPayContactCode checked it")
            };
            self.used = true;
            let source = AutoAcceptProofSource::SignWithKey {
                secret_key,
                expiry: self.expiry,
            };
            Ok((source, ScanSpend { _claim: self }))
        }
    }

    impl Drop for ScanClaim {
        /// Releases the scan, or spends it once its key went out.
        fn drop(&mut self) {
            let mut store = guard(&self.scans.0);
            if let Some(at) = store.position(self.wallet, &self.id) {
                if self.used {
                    store.scans.remove(at);
                } else {
                    store.scans[at].1.claimed = false;
                }
            }
        }
    }

    impl Scans {
        /// Claims `wallet`'s scan `id` for `recipient`. Unknown, spent,
        /// another wallet's or expired by `now`: `contact.scan_expired`
        /// (m4 §2.4), and an expired scan goes. Scanned for someone else:
        /// `invalid_argument`. Held by another send:
        /// `contact.request_pending`.
        pub(crate) fn claim(
            self: &Arc<Self>,
            wallet: WalletId,
            id: &str,
            recipient: Identifier,
            now: u64,
        ) -> Result<ScanClaim, ContactError> {
            let mut store = guard(&self.0);
            let at = store
                .position(wallet, id)
                .ok_or(ContactError::ScanExpired)?;
            let scan = &mut store.scans[at].1;
            if scan.recipient != recipient {
                return Err(invalid("the scan is for another contact").into());
            }
            if scan.expired(now) {
                store.scans.remove(at);
                return Err(ContactError::ScanExpired);
            }
            if scan.claimed {
                return Err(ContactError::RequestPending);
            }
            scan.claimed = true;
            Ok(ScanClaim {
                scans: Arc::clone(self),
                wallet,
                id: id.to_owned(),
                expiry: scan.expiry,
                recipient_unverified: scan.recipient_unverified,
                used: false,
            })
        }
    }

    impl NetworkSession {
        /// What `send_request(identity, to, scan)` sends, with the scan
        /// claimed; DP2-02 checks eligibility and grants, sends it, then
        /// spends the claim (module doc).
        pub(crate) fn send_request_intent(
            &self,
            wallet: WalletId,
            sender: &str,
            to: &str,
            scan: Option<&str>,
        ) -> Result<SendRequestIntent, ContactError> {
            let (sender, recipient) = (identifier(sender)?, identifier(to)?);
            let now = crate::events::unix_now();
            let scans = &self.platform.users.scans;
            let auto_accept =
                (scan.map(|id| scans.claim(wallet, id, recipient, now))).transpose()?;
            Ok(SendRequestIntent {
                sender,
                recipient,
                auto_accept,
            })
        }
    }
}
