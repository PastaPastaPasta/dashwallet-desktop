//! The DashPay facade (DASHPAY §2.4, §3.6): plain Rust that `dwcli` drives
//! directly and the UI binding (E0-13) wraps one to one. The contract is
//! `docs/contracts/m4-dashpay-engine.md`; its §3 listing is generated from
//! the facade's files and checked by `tests/m4_dashpay_contract.rs`.
//!
//! This file holds the handle and what every domain shares. Each domain file
//! (`startup.rs`, `identity.rs`, `registration.rs`, …) holds its records and
//! its own `impl DashPay` block, so the DP tasks edit disjoint files.
//!
//! E0-08 is the skeleton: every call returns `platform.not_implemented` with
//! its name. The DP tasks fill in the bodies without changing a signature,
//! or update the contract in the same change.

use std::sync::Arc;

use zeroize::{Zeroize, Zeroizing};

use super::errors::PlatformError;
use crate::{NetworkSession, WalletId};

/// DashPay for one wallet of a network session. A stateless handle: state
/// that outlives a call (avatar candidates, scan proofs, caches) lives in
/// the session, so any number of handles may exist for one wallet.
pub struct DashPay {
    pub(super) session: Arc<NetworkSession>,
    wallet_id: WalletId,
}

impl NetworkSession {
    /// The DashPay facade for `wallet_id`. Cheap; the calls check the wallet.
    pub fn dashpay(self: &Arc<Self>, wallet_id: WalletId) -> Arc<DashPay> {
        Arc::new(DashPay {
            session: Arc::clone(self),
            wallet_id,
        })
    }
}

impl DashPay {
    pub fn wallet_id(&self) -> WalletId {
        self.wallet_id
    }
}

/// The skeleton's body for every call.
pub(super) fn stub<T, E: From<PlatformError>>(call: &str) -> Result<T, E> {
    Err(PlatformError::NotImplemented { call: call.into() }.into())
}

/// A bearer credential handed in by the host: an invitation link, a scanned
/// payload that may carry a `dapk` (§3.8). It never serializes, `Debug`
/// never shows it, it cannot be cloned or compared, and it is zeroed on
/// drop. A binding builds it with [`BearerSecret::from_utf8`] from bytes it
/// received as bytes, never as a host string.
#[derive(serde::Deserialize)]
#[serde(from = "String")]
pub struct BearerSecret(Zeroizing<String>);

impl BearerSecret {
    pub fn new(secret: String) -> Self {
        Self(Zeroizing::new(secret))
    }

    /// Takes the bytes without copying them. Invalid UTF-8 is zeroed and
    /// refused with an `invalid_argument` that does not quote it.
    pub fn from_utf8(mut bytes: Zeroizing<Vec<u8>>) -> Result<Self, PlatformError> {
        match String::from_utf8(std::mem::take(&mut *bytes)) {
            Ok(secret) => Ok(Self::new(secret)),
            Err(e) => {
                e.into_bytes().zeroize();
                Err(PlatformError::InvalidArgument {
                    detail: "bearer secret is not UTF-8".into(),
                })
            }
        }
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl From<String> for BearerSecret {
    fn from(secret: String) -> Self {
        Self::new(secret)
    }
}

impl std::fmt::Debug for BearerSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BearerSecret(..)")
    }
}
