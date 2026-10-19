//! Requests through a session's own SDK, built from its options as every
//! session's is, for other crates' tests (MG-03 D37). Compiled only with
//! the `test-support` feature in a debug build: dwcli's dev-dependency is
//! all that enables the feature, so any debug build of dw-engine with it
//! has this read-only API (hidden from its docs), and a release or dist
//! build, which has no `debug_assertions`, never does (D40).

use dash_sdk::platform::Fetch;
use dash_sdk::query_types::IdentityBalance;
use dpp::prelude::Identifier;

use crate::{EngineError, NetworkSession};

/// The credits read's request (an identity balance, MG-03 §6) with no
/// masternode list: whether it succeeded.
pub async fn fetch_identity_balance(
    session: &NetworkSession,
    identity: [u8; 32],
) -> Result<bool, EngineError> {
    let sdk = session.manager()?.sdk_arc();
    let fetch = async move {
        Ok(IdentityBalance::fetch(&sdk, Identifier::from(identity))
            .await
            .is_ok())
    };
    session.on_runtime(fetch).await
}
