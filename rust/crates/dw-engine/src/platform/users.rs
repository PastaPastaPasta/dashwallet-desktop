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
//! - A DIP-15 code (`dash:?du=&dapk=`) carries the QR owner's auto-accept
//!   key. It stays here as a scan for DP2-02's send, which signs the proof
//!   with it and spends the scan once sent ([`send_seam`]); the host gets
//!   only the scan id.
//!   Neither app makes such codes yet (DEC-15).

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use dash_sdk::drive::config::DEFAULT_QUERY_LIMIT;
use dash_sdk::platform::dpns_usernames::is_valid_username;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dpp::util::strings::convert_to_homograph_safe_chars;
use dw_uri::deeplink::{DashPayContactCode, DashPayUserLink};
use platform_wallet::wallet::identity::crypto::auto_accept::decode_auto_accept_key_blob;
use zeroize::Zeroizing;

use super::contacts::ScannedContact;
use super::contacts_model::identifier;
use super::errors::{ContactError, IdentityError, PlatformError};
use super::names::UserHit;
use super::reverify::{Held, Refetcher, SdkRefetcher, ShownProfile};
use super::runtime::guard;
use super::trust::{self, BoxFuture};
use crate::{NetworkSession, WalletId};
use send_seam::{AutoAcceptKey, Scan};

/// Scans a session keeps; the oldest goes first.
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
    pub(super) scans: Scans,
    #[cfg(test)]
    pub(super) net: Mutex<Option<Arc<dyn UserNet>>>,
}

/// The session's scans by id, oldest first. Single-use: a send spends one.
#[derive(Default)]
pub(crate) struct Scans(Mutex<VecDeque<(String, Scan)>>);

impl Scans {
    /// Keeps `scan`, dropping the expired ones (their keys are of no use).
    fn insert(&self, scan: Scan, now: u64) -> String {
        let id = format!("scan-{}", hex::encode(rand::random::<[u8; 12]>()));
        let mut scans = guard(&self.0);
        scans.retain(|(_, s)| !s.key.expired(now));
        if scans.len() == MAX_SCANS {
            scans.pop_front();
        }
        scans.push_back((id.clone(), scan));
        id
    }

    fn remove(&self, id: &str) {
        guard(&self.0).retain(|(i, _)| i != id);
    }

    /// Drops the wallet's scans (removal, unload).
    pub(crate) fn forget(&self, wallet: &WalletId) {
        guard(&self.0).retain(|(_, s)| s.wallet != *wallet);
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        guard(&self.0).len()
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
        Ok(self.lookup(wallet, &username).await?.map(|(_, hit)| hit))
    }

    /// The user who owns `username` now, with the hit shown for them. A
    /// label DPNS could not hold asks nothing.
    async fn lookup(
        &self,
        wallet: WalletId,
        username: &str,
    ) -> Result<Option<(Identifier, UserHit)>, PlatformError> {
        let name = username.trim();
        let label = match name.len().checked_sub(5) {
            Some(i) if name.is_char_boundary(i) && name[i..].eq_ignore_ascii_case(".dash") => {
                &name[..i]
            }
            _ => name,
        };
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
        self.require_wallet(&wallet).map_err(PlatformError::from)?;
        if let Some(link) = DashPayUserLink::parse(text) {
            let claimed = Identifier::from(link.identity_id);
            return match self.lookup(wallet, &link.username).await? {
                Some((owner, hit)) if owner == claimed => Ok(scanned(hit, None)),
                _ => Err(ContactError::ScanUnverified),
            };
        }
        let not_a_code = || ContactError::from(invalid("not a DashPay contact code"));
        let code = DashPayContactCode::parse(text).ok_or_else(not_a_code)?;
        // The expiry is the key's hardened derivation index (DIP-15), so the
        // owner can verify no proof whose expiry is past 2^31 − 1.
        let (key, expiry) = (decode_auto_accept_key_blob(&code.key_blob).ok())
            .filter(|(_, expiry)| *expiry < 1 << 31)
            .ok_or_else(not_a_code)?;
        let now = crate::events::unix_now();
        let key = AutoAcceptKey {
            key: Zeroizing::new(key.to_secret_bytes()),
            expiry,
            recipient_unverified: false,
        };
        if key.expired(now) {
            return Err(ContactError::ScanExpired);
        }
        let (recipient, hit) =
            (self.lookup(wallet, &code.username).await?).ok_or(ContactError::ScanUnverified)?;
        let key = AutoAcceptKey {
            recipient_unverified: hit.unverified,
            ..key
        };
        let scans = &self.platform.users.scans;
        let scan = scans.insert(
            Scan {
                wallet,
                recipient,
                key,
            },
            now,
        );
        // A removal or unload since the check above took the wallet's scans
        // (after the wallet left the manager); this one must not outlive it.
        if let Err(e) = self.require_wallet(&wallet) {
            scans.remove(&scan);
            return Err(PlatformError::from(e).into());
        }
        Ok(scanned(hit, Some(scan)))
    }
}

fn scanned(hit: UserHit, scan: Option<String>) -> ScannedContact {
    ScannedContact {
        identity: hit.identity,
        username: Some(hit.username),
        display_name: hit.display_name,
        scan,
        relation: hit.relation,
        unverified: hit.unverified,
    }
}

/// The seam DP2-02's `send_request` sends through: the request's sides and,
/// from a scan, the auto-accept key. platform-wallet's
/// `send_contact_request_from_qr` would parse the code and resolve its name
/// again at send time, so the send uses
/// `send_contact_request_with_external_signer` with
/// [`AutoAcceptKey::proof_source`] instead: the recipient is the identity
/// `verify_scanned` showed, and the proof is signed once the send knows its
/// `accountReference`.
#[cfg_attr(not(test), expect(dead_code, reason = "DP2-02's send_request"))]
pub(crate) mod send_seam {
    use dashcore::secp256k1::SecretKey;
    use dpp::prelude::Identifier;
    use platform_wallet::AutoAcceptProofSource;
    use zeroize::Zeroizing;

    use super::super::contacts_model::identifier;
    use super::{Scans, invalid};
    use crate::platform::errors::ContactError;
    use crate::platform::runtime::guard;
    use crate::{NetworkSession, WalletId};

    /// A verified `dapk` code: whom it lets this wallet's request reach
    /// auto-accepted, and the key that signs the proof.
    pub(crate) struct Scan {
        pub(super) wallet: WalletId,
        pub(super) recipient: Identifier,
        pub(super) key: AutoAcceptKey,
    }

    /// One contact request to send.
    pub(crate) struct SendRequestIntent {
        pub(crate) sender: Identifier,
        pub(crate) recipient: Identifier,
        /// From a `dapk` scan: the proof's key. The send spends the scan
        /// ([`Scans::spend`]) once the request was broadcast, or may have
        /// been; a send that fails before that leaves it for a retry.
        pub(crate) auto_accept: Option<AutoAcceptKey>,
    }

    /// A scanned code's auto-accept key.
    #[derive(Clone)]
    pub(crate) struct AutoAcceptKey {
        pub(super) key: Zeroizing<[u8; 32]>,
        /// When the key stops being honoured: UNIX seconds.
        pub(crate) expiry: u32,
        /// The lookup that named the recipient did not verify (m4 §5): the
        /// send confirms before it auto-accepts with Platform's word alone.
        pub(crate) recipient_unverified: bool,
    }

    impl AutoAcceptKey {
        /// The proof the send signs over its own `accountReference`.
        pub(crate) fn proof_source(&self) -> AutoAcceptProofSource {
            let secret_key = SecretKey::from_secret_bytes(*self.key)
                .expect("a valid scalar: decode_auto_accept_key_blob checked it");
            AutoAcceptProofSource::SignWithKey {
                secret_key,
                expiry: self.expiry,
            }
        }

        pub(super) fn expired(&self, now: u64) -> bool {
            u64::from(self.expiry) <= now
        }
    }

    impl Scans {
        /// The key of `wallet`'s scan `id` for `recipient`, which stays until
        /// spent. Unknown, spent, another wallet's or expired by `now`:
        /// `contact.scan_expired` (m4 §2.4), and an expired scan goes.
        /// Scanned for someone else: `invalid_argument`.
        pub(crate) fn key(
            &self,
            wallet: WalletId,
            id: &str,
            recipient: Identifier,
            now: u64,
        ) -> Result<AutoAcceptKey, ContactError> {
            let mut scans = guard(&self.0);
            let at = (scans.iter())
                .position(|(i, s)| i == id && s.wallet == wallet)
                .ok_or(ContactError::ScanExpired)?;
            let scan = &scans[at].1;
            if scan.recipient != recipient {
                return Err(invalid("the scan is for another contact").into());
            }
            if scan.key.expired(now) {
                scans.remove(at);
                return Err(ContactError::ScanExpired);
            }
            Ok(scan.key.clone())
        }

        /// Spends `wallet`'s scan `id`: a request was sent with it.
        pub(crate) fn spend(&self, wallet: WalletId, id: &str) {
            guard(&self.0).retain(|(i, s)| !(i == id && s.wallet == wallet));
        }
    }

    impl NetworkSession {
        /// What `send_request(identity, to, scan)` sends; DP2-02 checks
        /// eligibility and grants, sends it, then spends the scan.
        pub(crate) fn send_request_intent(
            &self,
            wallet: WalletId,
            sender: &str,
            to: &str,
            scan: Option<&str>,
        ) -> Result<SendRequestIntent, ContactError> {
            let (sender, recipient) = (identifier(sender)?, identifier(to)?);
            let now = crate::events::unix_now();
            let auto_accept = (scan
                .map(|id| (self.platform.users.scans).key(wallet, id, recipient, now)))
            .transpose()?;
            Ok(SendRequestIntent {
                sender,
                recipient,
                auto_accept,
            })
        }
    }
}
