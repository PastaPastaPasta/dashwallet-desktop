//! Transaction history and the detail sheet.

use crate::dto::history::{HistoryRequest, HistoryRows, TxDetails};
use crate::ipc::commands;
use crate::state::parse_wallet_id;

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

commands! {
    #[facade("NetworkSession::history_page")]
    fn history_page(state, wallet_id: String, request: HistoryRequest) -> HistoryRows {
        let id = parse_wallet_id(&wallet_id)?;
        let session = state.session().await?;
        Ok(session.history_page(id, request.into_query()?).await?.into())
    }

    #[facade("NetworkSession::tx_detail")]
    fn tx_detail(state, wallet_id: String, txid: String) -> TxDetails {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state.session().await?.tx_detail(id, txid).await?.into())
    }
}
