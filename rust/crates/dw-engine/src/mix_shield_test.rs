//! Test-only calls for the regtest suite of "move mixed coins"
//! (`regtest/harness/tests/test_move_mixed.py`, dwcli's `mix-shield-test`
//! feature). Compiled only in a debug build with `test-mix-shield`;
//! `tests/mix_shield_release_check.rs` proves the module absent from a
//! release build.

use crate::{EngineError, NetworkSession, WalletId};

impl crate::TestSupportProbe {
    /// Present only where this module is compiled.
    pub const MIX_SHIELD_TEST: bool = true;
}

impl NetworkSession {
    /// `count` fresh addresses of the wallet's CoinJoin account (external
    /// chain), for a test to fund the account without mixing.
    pub async fn coinjoin_addresses_for_test(
        &self,
        wallet_id: WalletId,
        count: usize,
    ) -> Result<Vec<String>, EngineError> {
        Ok(self
            .reserve_coinjoin_addresses(wallet_id, count, false)
            .await?
            .iter()
            .map(ToString::to_string)
            .collect())
    }
}
