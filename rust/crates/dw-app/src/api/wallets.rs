//! Wallets: the list, balances.

use crate::dto::{Balances, WalletSummary};
use crate::ipc::commands;
use crate::state::parse_wallet_id;

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

commands! {
    #[facade("NetworkSession::wallet_infos")]
    fn list_wallets(state) -> Vec<WalletSummary> {
        let infos = state.session().await?.wallet_infos()?;
        Ok(infos.into_iter().map(WalletSummary::from).collect())
    }

    /// Balance buckets; `null` until the scan reaches the birth height.
    #[facade("NetworkSession::balances")]
    fn balance(state, wallet_id: String) -> Option<Balances> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state.session().await?.balances(&id)?.map(Balances::from))
    }
}
