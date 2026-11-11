//! What a masternode withdrawal asks of Platform (MG-13 F5, P7, P8, P12),
//! behind a seam as DP6-02's `WithdrawNet`: [`SdkMnWithdrawals`] asks the
//! SDK; the tests script it.
//!
//! Every read is proved and comes back as [`Proved`]: its proof's Platform
//! height, the proof metadata's `core_chain_locked_height` (the Core height
//! Platform last committed, quorum-signed through its `StateId`, F8), its
//! block time, and its provenance (P12): verified iff every quorum key its
//! proof used was SPV-verified, counted by `trust::scoped` around the
//! `fetch_with_metadata` (`reverify.rs` reads the tally the same way). The
//! `Refetcher` is not used: it drops the metadata P4 (a) and the floors
//! need. No read uses the SDK's nonce cache.

use std::future::Future;
use std::sync::Arc;

use dash_sdk::dapi_grpc::platform::v0::ResponseMetadata;
use dash_sdk::platform::Fetch;
use dash_sdk::platform::transition::broadcast::BroadcastStateTransition;
use dash_sdk::query_types::{IdentityBalance, IdentityNonceFetcher};
use dpp::consensus::ConsensusError;
use dpp::consensus::codes::ErrorWithCode;
use dpp::consensus::state::state_error::StateError;
use dpp::document::Document;
use dpp::identity::Identity;
use dpp::prelude::Identifier;
use dpp::serialization::PlatformSerializable;
use dpp::state_transition::StateTransition;
use dpp::version::PlatformVersion;

use crate::platform::PlatformError;
use crate::platform::trust;
use crate::platform::withdrawal::withdrawal_query;

/// A proved read (module doc).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Proved<T> {
    pub(crate) value: T,
    /// The proof's Platform block height.
    pub(crate) height: u64,
    /// The proof's `core_chain_locked_height` (F8, P4 (a)).
    pub(crate) core_height: u32,
    /// The proof's block time, milliseconds.
    pub(crate) time_ms: u64,
    /// Every quorum key the proof used was SPV-verified (P12).
    pub(crate) verified: bool,
}

impl<T> Proved<T> {
    pub(crate) fn map<U>(self, f: impl FnOnce(T) -> U) -> Proved<U> {
        Proved {
            value: f(self.value),
            height: self.height,
            core_height: self.core_height,
            time_ms: self.time_ms,
            verified: self.verified,
        }
    }
}

/// What one broadcast's answer says (P7). Only a CheckTx consensus refusal
/// that is no nonce error is `Refused`; nothing here is definitive: the
/// record stays open whatever it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Answer {
    /// CheckTx took it.
    Accepted,
    /// "tx already exists in cache": the bytes are in a mempool already
    /// (rs-dapi `error_mapping.rs:386-387` → gRPC `AlreadyExists` →
    /// `dash_sdk::Error::AlreadyExists`, `rs-sdk/src/error.rs:237-238`).
    InCache,
    /// `InvalidIdentityNonceError` (code 40204,
    /// `rs-dpp/src/errors/consensus/codes.rs:391`): the nonce is used, or
    /// out of Drive's window; says nothing about which transition used it.
    NonceRefused(String),
    /// Another consensus refusal: its code and message.
    Refused(String),
    /// No verdict (transport, timeout, a node's own failure).
    Unknown(String),
}

/// The seam (module doc).
pub(crate) trait MnWithdrawNet: Send + Sync {
    /// The network's protocol version, as the SDK last proved it (read it
    /// after a proved read, as DP6-02 does).
    fn version(&self) -> &'static PlatformVersion;
    /// The masternode's owner identity with every key, disabled ones
    /// included; `None` when Platform has none.
    fn identity(
        &self,
        id: Identifier,
    ) -> impl Future<Output = Result<Proved<Option<Identity>>, PlatformError>> + Send;
    /// The identity's nonce as Drive stores it (the newest and the missing
    /// bits above it; 0 when none).
    fn nonce_state(
        &self,
        id: Identifier,
    ) -> impl Future<Output = Result<Proved<u64>, PlatformError>> + Send;
    /// The identity's credits; `None` when Platform has no identity.
    fn balance(
        &self,
        id: Identifier,
    ) -> impl Future<Output = Result<Proved<Option<u64>>, PlatformError>> + Send;
    /// The withdrawal document `id`, present or proved absent.
    fn document(
        &self,
        id: Identifier,
    ) -> impl Future<Output = Result<Proved<Option<Document>>, PlatformError>> + Send;
    /// Sends `bytes`, a stored signed transition, unchanged: no signing,
    /// no nonce, no SDK cache.
    fn broadcast(&self, bytes: &[u8]) -> impl Future<Output = Answer> + Send;
}

pub(crate) struct SdkMnWithdrawals(pub(crate) Arc<dash_sdk::Sdk>);

/// `fetch` in its own trust scope (P12).
async fn proved<T>(
    fetch: impl Future<Output = Result<(T, ResponseMetadata), dash_sdk::Error>>,
) -> Result<Proved<T>, PlatformError> {
    let (fetched, tally) = trust::scoped(fetch).await;
    let (value, at) = fetched?;
    Ok(Proved {
        value,
        height: at.height,
        core_height: at.core_chain_locked_height,
        time_ms: at.time_ms,
        verified: tally.verified(),
    })
}

impl MnWithdrawNet for SdkMnWithdrawals {
    fn version(&self) -> &'static PlatformVersion {
        self.0.version()
    }

    async fn identity(&self, id: Identifier) -> Result<Proved<Option<Identity>>, PlatformError> {
        proved(Identity::fetch_with_metadata(&self.0, id, None)).await
    }

    async fn nonce_state(&self, id: Identifier) -> Result<Proved<u64>, PlatformError> {
        let read = proved(IdentityNonceFetcher::fetch_with_metadata(&self.0, id, None)).await?;
        Ok(read.map(|n| n.map_or(0, |IdentityNonceFetcher(n)| n)))
    }

    async fn balance(&self, id: Identifier) -> Result<Proved<Option<u64>>, PlatformError> {
        proved(IdentityBalance::fetch_with_metadata(&self.0, id, None)).await
    }

    async fn document(&self, id: Identifier) -> Result<Proved<Option<Document>>, PlatformError> {
        proved(Document::fetch_with_metadata(
            &self.0,
            withdrawal_query(&id)?,
            None,
        ))
        .await
    }

    /// The SDK's `broadcast` sends `serialize_to_bytes()` of the transition
    /// (`rs-sdk/.../broadcast_request.rs:68-74`) and nothing else: no
    /// signing, no nonce. The stored bytes are decoded and sent only if
    /// they encode back to themselves, so what leaves is them.
    async fn broadcast(&self, bytes: &[u8]) -> Answer {
        let decoded = StateTransition::deserialize_from_bytes_untrusted_exact_in_version(
            bytes,
            self.version(),
        );
        let transition = match decoded {
            Ok(t) if t.serialize_to_bytes().is_ok_and(|again| again == bytes) => t,
            Ok(_) => return Answer::Unknown("the stored bytes do not encode back".into()),
            Err(e) => return Answer::Unknown(format!("the stored bytes do not decode: {e}")),
        };
        classify(transition.broadcast(&self.0, None).await)
    }
}

/// P7's reading of a CheckTx answer (`platform-wallet`
/// `broadcast_outcome.rs:30-37`'s two consensus shapes, through the retry
/// envelopes).
pub(crate) fn classify(result: Result<(), dash_sdk::Error>) -> Answer {
    let e = match result {
        Ok(()) => return Answer::Accepted,
        Err(e) => e,
    };
    let mut leaf = &e;
    while let dash_sdk::Error::NoAvailableAddressesToRetry(inner) = leaf {
        leaf = inner;
    }
    match leaf {
        dash_sdk::Error::AlreadyExists(_) => Answer::InCache,
        dash_sdk::Error::Protocol(dpp::ProtocolError::ConsensusError(c)) => consensus(c),
        dash_sdk::Error::StateTransitionBroadcastError(b) => match &b.cause {
            Some(c) => consensus(c),
            None => Answer::Unknown(e.to_string()),
        },
        _ => Answer::Unknown(e.to_string()),
    }
}

fn consensus(c: &ConsensusError) -> Answer {
    let text = format!("{} {c}", c.code());
    match c {
        ConsensusError::StateError(StateError::InvalidIdentityNonceError(_)) => {
            Answer::NonceRefused(text)
        }
        _ => Answer::Refused(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scripted fetch answering at height 7 after quorum lookups, each
    /// SPV-verified or not, as a proof's would be.
    async fn fetched(lookups: &[bool]) -> Proved<u8> {
        let lookups = lookups.to_vec();
        proved(async move {
            for verified in lookups {
                trust::note_lookup(verified);
            }
            let at = ResponseMetadata {
                height: 7,
                core_chain_locked_height: 3,
                time_ms: 9,
                ..ResponseMetadata::default()
            };
            Ok::<_, dash_sdk::Error>((1u8, at))
        })
        .await
        .unwrap()
    }

    /// Sol (r1): the SDK adapter's provenance is its own trust scope's
    /// tally. A read is `verified` (Enforce passes it, and only it raises a
    /// floor; P5, P12) only when it used a quorum key and SPV verified
    /// every one; an outer scope's lookups don't count for it.
    #[tokio::test]
    async fn a_read_is_verified_only_when_its_scope_verified_every_quorum_key() {
        let read = fetched(&[true, true]).await;
        assert!(read.verified);
        assert_eq!(
            (read.value, read.height, read.core_height, read.time_ms),
            (1, 7, 3, 9)
        );
        assert!(!fetched(&[true, false]).await.verified);
        assert!(!fetched(&[false]).await.verified);
        assert!(!fetched(&[]).await.verified, "no quorum key: no proof");
        let ((), outer) = trust::scoped(async {
            trust::note_lookup(false);
            assert!(fetched(&[true]).await.verified);
        })
        .await;
        assert!(!outer.verified(), "the inner read still counts outside");
    }
}
