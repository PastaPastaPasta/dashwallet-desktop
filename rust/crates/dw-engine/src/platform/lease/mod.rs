//! Leases, the lock coordinator and the dispatch fence (E0-04 design §4,
//! §5, §8; P2a: mode-independent engine side).
//!
//! - [`table`]: the lease table. Everything decided about leases, permits
//!   and artifacts happens in one critical section of its mutex J, a leaf
//!   that is never held across an await, I/O or a library lock (H1).
//! - [`budget`]: a purpose's budget with authority generations (§4.2).
//! - [`fence`]: `register`, `admit`, `abandon` and the attempt guards; the
//!   in-memory journal, the row-less set and its tombstones;
//!   `dispatch_status` (§5.4, §5.5, §16.6).
//! - [`lock`]: the freeze, the barrier, the per-request vault gate and the
//!   drain (§8.1–§8.3); the outcomes of a lock (§8.4).
//! - [`session`]: `NetworkSession`'s side: `begin_lease`, `lock_vault`, the
//!   revoking vault methods, the background lease and the facade's lease
//!   handle.
//!
//! The library side (Mode A's PR, Mode B's call permits) builds on this
//! API: P2b and P4 install it, DP1-02 uses it.

pub(crate) mod budget;
pub(crate) mod fence;
pub(crate) mod lock;
pub(crate) mod session;
pub(crate) mod table;

#[cfg(test)]
mod session_tests;
#[cfg(test)]
mod stress_tests;
#[cfg(test)]
mod tests;

use std::time::Duration;

use dw_vault::VaultStatus;

use super::errors::PlatformError;
use super::flows::{BudgetPurpose, FlowKind, RevokeCause};

pub use fence::{
    AdmitRequest, ArtifactKind, AttemptGuard, DispatchPermit, NonceSlot, NonceSpace, Outcome,
    Settlement, Verdict,
};
pub use table::{Lease, LeaseTable};

/// A lease's id: 128 bits from the OS RNG, in memory only (H5, I7).
pub type LeaseId = [u8; 16];

/// The facade's form of a lease id: what `begin_flow` returns and every
/// call taking a `grant` accepts. Vault grant ids are 32 hex digits, so the
/// prefix keeps the two apart.
pub(crate) const LEASE_PREFIX: &str = "lease-";

pub(crate) fn lease_string(id: &LeaseId) -> String {
    format!("{LEASE_PREFIX}{}", hex::encode(id))
}

pub(crate) fn parse_lease(s: &str) -> Option<LeaseId> {
    let hex_part = s.strip_prefix(LEASE_PREFIX)?;
    let mut id = [0u8; 16];
    hex::decode_to_slice(hex_part, &mut id).ok()?;
    Some(id)
}

/// A handed-off artifact: a txid or a state-transition hash, 32 bytes in
/// display order, so its string is plain lower-case hex (a txid as dash-qt
/// shows it).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArtifactId(pub [u8; 32]);

impl ArtifactId {
    pub fn from_txid(txid: &dashcore::Txid) -> Self {
        use dashcore::hashes::Hash;
        let mut bytes = txid.to_byte_array();
        bytes.reverse();
        Self(bytes)
    }

    pub fn parse(s: &str) -> Option<Self> {
        let mut bytes = [0u8; 32];
        hex::decode_to_slice(s, &mut bytes).ok()?;
        Some(Self(bytes))
    }
}

impl std::fmt::Display for ArtifactId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

impl std::fmt::Debug for ArtifactId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ArtifactId({})", &hex::encode(self.0)[..16])
    }
}

/// Hand-offs that run without a lease (§5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnleasedKind {
    /// `TxDraft` M1 sends, until P5 leases them.
    Send,
    /// CoinJoin denominations and collaterals.
    Mixing,
    /// PSBT bytes signed elsewhere.
    External,
    /// A resend of a transaction already in the wallet's records.
    Rebroadcast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Lease(LeaseId),
    Unleased(UnleasedKind),
}

/// Who is handing off, read by `register` and `admit` (§5.2). The engine
/// wraps every call that can hand off in one: [`Lease::scope`] or
/// [`DispatchScope::unleased`]. P3 moves the task-local into rs-sdk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchScope {
    pub wallet: crate::WalletId,
    pub origin: Origin,
    /// A resumable step (§7.6), e.g. `registration/<draft>/identity`.
    pub step: Option<String>,
}

tokio::task_local! {
    static SCOPE: DispatchScope;
}

impl DispatchScope {
    /// The scope of the running task, if any.
    pub fn current() -> Option<Self> {
        SCOPE.try_with(Clone::clone).ok()
    }

    pub async fn run<F: Future>(self, f: F) -> F::Output {
        SCOPE.scope(self, f).await
    }

    pub async fn unleased<F: Future>(
        wallet: crate::WalletId,
        kind: UnleasedKind,
        f: F,
    ) -> F::Output {
        Self {
            wallet,
            origin: Origin::Unleased(kind),
            step: None,
        }
        .run(f)
        .await
    }
}

/// A flow's or an artifact's outcome (§4.6, §8.4), weakest first: a flow
/// takes the strongest of its artifacts'.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FlowOutcome {
    /// Nothing of it was ever committed, or every committed row-less
    /// artifact settled definitely unsent.
    Cancelled,
    /// Handed off, outcome unknown.
    MaybeSent,
    /// Committed with a record; the engine sends it again.
    WillBeSent,
    Sent,
}

/// One revoked flow in a lock's report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowReport {
    /// The facade's lease id.
    pub lease: String,
    pub wallet_id: String,
    pub flow: FlowKind,
    pub outcome: FlowOutcome,
    /// Each committed artifact of the flow, with its own outcome.
    pub artifacts: Vec<(String, FlowOutcome)>,
}

/// What a lock found (§8.4): the vault after its gate, and every lease it
/// revoked or drained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockReport {
    pub status: VaultStatus,
    pub flows: Vec<FlowReport>,
}

/// `EngineEvent::LockProgress`'s phase (§16.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockPhase {
    /// Permits handed off before the lock are still in flight (C6).
    Draining {
        in_flight: u32,
        deadline_in_ms: u64,
    },
    Done(LockReport),
}

/// Why a lease refused a call (§16.4 maps each to its code).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LeaseError {
    /// A lock landed while the lease was being created, or Lock won the
    /// flow's permit: `platform.cancelled`.
    #[error("a lock landed first")]
    Locked,
    #[error("lease revoked: {0:?}")]
    Revoked(RevokeCause),
    /// Ended by `end_flow` or the idle reaper.
    #[error("lease ended")]
    Ended,
    /// The key window passed or the flow parked: it needs a fresh grant.
    #[error("lease parked; needs a grant for {0:?}")]
    Parked(BudgetPurpose),
    /// An epoch change, or the purpose's budget is 0.
    #[error("needs a grant for {0:?}")]
    NeedsGrant(BudgetPurpose),
    #[error("{purpose:?} budget exceeded: {needed} needed, {remaining} remaining")]
    Exceeded {
        purpose: BudgetPurpose,
        needed: u64,
        remaining: u64,
    },
    /// Unknown, used, of the wrong kind, or another wallet's.
    #[error("grant invalid")]
    Invalid,
    /// The vault refused for a reason the codes above do not cover.
    #[error("vault: {0}")]
    Vault(&'static str),
}

impl From<LeaseError> for PlatformError {
    fn from(e: LeaseError) -> Self {
        match e {
            LeaseError::Locked => PlatformError::Cancelled,
            LeaseError::Revoked(cause) => PlatformError::LeaseRevoked { cause },
            LeaseError::Ended => PlatformError::LeaseExpired,
            LeaseError::Parked(purpose) | LeaseError::NeedsGrant(purpose) => {
                PlatformError::NeedsGrant { purpose }
            }
            LeaseError::Exceeded {
                purpose,
                needed,
                remaining,
            } => PlatformError::GrantExceeded {
                purpose,
                needed,
                remaining,
            },
            LeaseError::Invalid => PlatformError::GrantInvalid,
            LeaseError::Vault(detail) => PlatformError::Internal {
                detail: detail.into(),
            },
        }
    }
}

/// The table's timing (§4.4, §4.7, §8.2). Tests shrink them.
#[derive(Debug, Clone, Copy)]
pub struct LeaseConfig {
    /// H: a permit's lifetime from its grant.
    pub permit_ttl: Duration,
    /// An own key without a proof wait lives this long from the lease's
    /// creation (the grant TTL).
    pub key_ttl: Duration,
    /// A vault-key lease with no call, permit or flow task is ended after
    /// this long.
    pub idle: Duration,
}

impl Default for LeaseConfig {
    fn default() -> Self {
        Self {
            permit_ttl: Duration::from_secs(10),
            key_ttl: Duration::from_secs(120),
            idle: Duration::from_secs(600),
        }
    }
}

#[cfg(test)]
mod id_tests {
    use super::*;

    #[test]
    fn lease_and_artifact_ids_round_trip() {
        let id = [7u8; 16];
        let s = lease_string(&id);
        assert!(s.starts_with(LEASE_PREFIX));
        assert_eq!(parse_lease(&s), Some(id));
        assert_eq!(
            parse_lease(&hex::encode(id)),
            None,
            "a grant id is not a lease"
        );
        let txid: dashcore::Txid =
            "0102030405060708091011121314151617181920212223242526272829303132"
                .parse()
                .unwrap();
        let a = ArtifactId::from_txid(&txid);
        assert_eq!(a.to_string(), txid.to_string(), "display order");
        assert_eq!(ArtifactId::parse(&a.to_string()), Some(a));
    }
}
