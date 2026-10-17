//! Debug builds' tests of other crates only (`test-support`, enabled by
//! dwcli's dev-dependency): requests through a session's own SDK, built
//! from its options as every session's is (MG-03 D37). Never in a release.

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
